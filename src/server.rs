use crate::message::{
    AddResponse, ClientMessage, ServerMessage
};

use crate::message::client_message::Message as ClientMessageEnum;
use crate::message::server_message::Message as ServerMessageEnum;

use log::{error, info, warn};
use prost::Message;
use std::sync::{Mutex, MutexGuard};
use std::{
    collections::HashMap,
    io::{self, ErrorKind, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

use lazy_static::lazy_static;

const MAX_MESSAGE_SIZE: usize = 1024 * 1024;

lazy_static! {
    static ref SERVERS: Arc<Mutex<HashMap<String, Arc<Server>>>> = Arc::new(Mutex::new(HashMap::new()));
}
#[derive(Debug)]
struct Client {
    stream: TcpStream,
}

impl Client {
    fn read_message(&mut self) -> io::Result<Vec<u8>> {
        let mut len_buf = [0u8; 4];
        self.stream.read_exact(&mut len_buf)?;

        let message_len = u32::from_be_bytes(len_buf) as usize;
        if message_len > MAX_MESSAGE_SIZE {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "Message size exceeds maximum allowed",
            ));
        }

        let mut buffer = vec![0; message_len];
        self.stream.read_exact(&mut buffer)?;
        Ok(buffer)
    }

    fn write_message(&mut self, payload: &[u8]) -> io::Result<()> {
        let len = payload.len() as u32;
        self.stream.write_all(&len.to_be_bytes())?;
        self.stream.write_all(payload)?;
        self.stream.flush()
    }

    pub fn new(stream: TcpStream) -> Self {
        Client { stream }
    }

    pub fn handle(&mut self) -> io::Result<bool> {
        self.stream.set_nonblocking(false)?;
        match self.read_message() {
            Ok(buffer) => match ClientMessage::decode(&buffer[..]) {
                Ok(client_msg) => {
                    if let Some(response) = self.process_message(client_msg.message) {
                        let encoded = response.encode_to_vec();
                        self.write_message(&encoded)?;
                    } else {
                        warn!("Received empty message");
                    }
                    Ok(true)
                }
                Err(e) => {
                    error!("Failed to decode message: {}", e);
                    Ok(false)
                }
            },
            Err(e) => {
                if e.kind() == ErrorKind::UnexpectedEof {
                    Ok(false)
                } else {
                    Err(e)
                }
            }
        }
    }

    fn process_message(
        &mut self,
        message: Option<ClientMessageEnum>,
    ) -> Option<ServerMessage> {
        match message {
            Some(ClientMessageEnum::EchoMessage(echo)) => {
                info!("Handling echo message: {}", echo.content);
                Some(ServerMessage {
                    message: Some(ServerMessageEnum::EchoMessage(echo)),
                })
            }
            Some(ClientMessageEnum::AddRequest(add)) => {
                info!("Handling add request: {} + {}", add.a, add.b);
                let result = add.a + add.b;
                Some(ServerMessage {
                    message: Some(ServerMessageEnum::AddResponse(AddResponse { result })),
                })
            }
            None => {
                warn!("No message found in ClientMessage.");
                None
            }
        }
    }
}


#[derive(Debug)]
pub struct Server {
    listener: TcpListener,
    is_running: Arc<AtomicBool>,
    client_cnt: Arc<Mutex<usize>>,
}

impl Server {
    /// Creates a new server instance
    pub fn new(addr: &str) -> io::Result<Arc<Self>> {
        let servers_lock = SERVERS.lock().unwrap();
        info!("Server instances: {:?}", *servers_lock);

        if let Some(server) = servers_lock.get(addr) {
            warn!("Server address {} already in use.", addr);
            Self::increment_client_count(server);
            return Ok(Arc::clone(server));
        }

        Self::create_and_register_server(addr, servers_lock)
    }

    fn increment_client_count(server: &Arc<Server>) {
        let mut cnt = server.client_cnt.lock().unwrap();
        *cnt += 1;
    }

    fn create_and_register_server(
        addr: &str,
        mut servers_lock: MutexGuard<'_, HashMap<String, Arc<Server>>>,
    ) -> io::Result<Arc<Self>> {
        match TcpListener::bind(addr) {
            Ok(listener) => {
                let server = Arc::new(Server {
                    listener,
                    is_running: Arc::new(AtomicBool::new(false)),
                    client_cnt: Arc::new(Mutex::new(1)),
                });
                servers_lock.insert(addr.to_string(), Arc::clone(&server));
                Ok(server)
            }
            Err(e) => {
                let msg = format!(
                    "Failed to bind to address {}: {}",
                    addr,
                    if e.kind() == ErrorKind::AddrInUse {
                        "Address is already in use"
                    } else {
                        "Unknown error"
                    }
                );
                error!("{}", msg);
                Err(io::Error::new(e.kind(), msg))
            }
        }
    }

    /// Runs the server, listening for incoming connections and handling them
    pub fn run(&self) -> io::Result<()> {
        self.start();
        self.listener.set_nonblocking(true)?;

        while self.is_running.load(Ordering::SeqCst) {
            self.accept_client()?;
        }

        info!("Server stopped.");
        Ok(())
    }

    fn start(&self) {
        self.is_running.store(true, Ordering::SeqCst);
        info!("Server is running on {}", self.listener.local_addr().unwrap());
    }

    fn accept_client(&self) -> io::Result<()> {
        match self.listener.accept() {
            Ok((stream, addr)) => {
                info!("New client connected: {}", addr);
                self.handle_client_connection(stream);
            }
            Err(ref e) if e.kind() == ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                error!("Error accepting connection: {}", e);
                return Err(e);
            }
        }
        Ok(())
    }

    fn handle_client_connection(&self, stream: TcpStream) {
        let is_running = Arc::clone(&self.is_running);

        thread::spawn(move || {
            let mut client = Client::new(stream);
            while is_running.load(Ordering::SeqCst) {
                if let Err(e) = client.handle() {
                    error!("Error handling client: {}", e);
                    break;
                }
            }
        });
    }

    /// Stops the server by setting the `is_running` flag to `false`
    pub fn stop(&self) {
        let mut cnt = self.client_cnt.lock().unwrap();
        if *cnt == 1 {
            self.shutdown();
        } else {
            *cnt -= 1;
            info!("Client disconnected. Remaining clients: {}", *cnt);
        }
    }

    fn shutdown(&self) {
        if self.is_running.load(Ordering::SeqCst) {
            self.is_running.store(false, Ordering::SeqCst);
            info!("Shutdown signal sent.");

            let mut server_lock = SERVERS.lock().unwrap();
            let addr = self.listener.local_addr().unwrap().to_string();
            server_lock.remove(&addr);
        } else {
            warn!("Server was already stopped or not running.");
        }
    }
}
