use crate::message::{
    AddResponse, ClientMessage, EchoMessage, ServerMessage
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

lazy_static! {
    static ref SERVERS: Arc<Mutex<HashMap<String, Arc<Server>>>> = Arc::new(Mutex::new(HashMap::new()));
}
#[derive(Debug)]
struct Client {
    stream: TcpStream,
}

impl Client {
    pub fn new(stream: TcpStream) -> Self {
        Client { stream }
    }

    pub fn handle(&mut self) -> io::Result<()> {
        let mut buffer = [0; 512];
        // Read data from the client
        let bytes_read = self.stream.read(&mut buffer)?;
        if bytes_read == 0 {
            info!("Client disconnected.");
            return Ok(());
        }

        match ClientMessage::decode(&buffer[..bytes_read]) {
            Ok(client_message) => {
                info!("Decoded message from client: {:?}", client_message);
                if let Some(client_msg) = client_message.message {
                    let response_message = self.process_message(client_msg);
                    self.send_response(ServerMessage { message: Some(response_message) })?;
                } else {
                    warn!("Received an empty message.");
                }
            }
            Err(err) => {
                error!("Failed to decode ClientMessage. Error: {:?}, Data: {:?}", err, &buffer[..bytes_read]);
            }
        }

        Ok(())
    }

    fn process_message(&self, client_msg: ClientMessageEnum) -> ServerMessageEnum {
        match client_msg {
            ClientMessageEnum::EchoMessage(echo) => {
                info!("Processing EchoMessage.");
                ServerMessageEnum::EchoMessage(EchoMessage { content: echo.content })
            }
            ClientMessageEnum::AddRequest(add) => {
                let result = add.a + add.b;
                info!("Processing AddRequest. Result: {}", result);
                ServerMessageEnum::AddResponse(AddResponse { result })
            }
        }
    }

    fn send_response(&mut self, response: ServerMessage) -> io::Result<()> {
        let mut buf = Vec::new();
        response.encode(&mut buf)?;
        self.stream.write_all(&buf)?;
        Ok(())
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
        let mut servers_lock = SERVERS.lock().unwrap();
        info!("Server instances: {:?}", *servers_lock);

        if let Some(server) = servers_lock.get(addr) {
            warn!("Server address {} already in use.", addr);
            {
                let mut  cnt = server.client_cnt.lock().unwrap();
                *cnt += 1;
            }
            return  Ok(Arc::clone(server));
        }
        match TcpListener::bind(addr) {
            Ok(listener) => {
                let is_running = Arc::new(AtomicBool::new(false));
                let client_cnt = Arc::new(Mutex::new(1));
                let server = Arc::new(Server {
                    listener,
                    is_running,
                    client_cnt,
                });
                servers_lock.insert(addr.to_string(), Arc::clone(&server));
                Ok(server)
            }
            Err(ref e) if e.kind() == ErrorKind::AddrInUse => {
                eprintln!("Address {} is already in use.", addr);
                Err(io::Error::new(e.kind(), e.to_string()))
            }
            Err(e) => {
                eprintln!("Failed to bind to address {}: {}", addr, e);
                Err(e)
            }    
        }
    }


    /// Runs the server, listening for incoming connections and handling them
    pub fn run(&self) -> io::Result<()> {
        self.is_running.store(true, Ordering::SeqCst); // Set the server as running
        info!("Server is running on {}", self.listener.local_addr()?);

        // Set the listener to non-blocking mode
        self.listener.set_nonblocking(true)?;

        while self.is_running.load(Ordering::SeqCst) {
            match self.listener.accept() {
                Ok((stream, addr)) => {
                    info!("New client connected: {}", addr);

                    // Handle the client request
                    let is_running = Arc::clone(&self.is_running);
        
                    // Spawn a new thread to handle the client connection
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
                Err(ref e) if e.kind() == ErrorKind::WouldBlock => {
                    // No incoming connections, sleep briefly to reduce CPU usage
                    thread::sleep(Duration::from_millis(100));
                }
                Err(e) => {
                    error!("Error accepting connection: {}", e);
                }
            }
        }

        info!("Server stopped.");
        Ok(())
    }

    /// Stops the server by setting the `is_running` flag to `false`
    pub fn stop(&self) {
        let mut cnt = self.client_cnt.lock().unwrap();
        if *cnt == 1 {
            if self.is_running.load(Ordering::SeqCst) {
                self.is_running.store(false, Ordering::SeqCst);
                info!("Shutdown signal sent.");

                let mut server_lock:MutexGuard<'_, HashMap<String, Arc<Server>>> = SERVERS.lock().unwrap();
                let addr = self.listener.local_addr().unwrap().to_string();
                server_lock.remove(&addr);
            } else {
                warn!("Server was already stopped or not running.");
            }
        } else {
            {
                *cnt -= 1;
                info!("Client disconnected.");
            }
            info!("Clients: {}", *cnt)
        }
        
    }
}
