use crate::message::{
    AddResponse, ClientMessage, EchoMessage, ServerMessage
};

use crate::message::client_message::Message as ClientMessageEnum;
use crate::message::server_message::Message as ServerMessageEnum;


use log::{error, info, warn};
use prost::Message;
use std::{
    io::{self, ErrorKind, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

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

       // Decode the incoming message
       if let Ok(client_message) = ClientMessage::decode(&buffer[..bytes_read]) {
        info!("Decoded message from client: {:?}", client_message);
        if let Some(client_msg) = client_message.message {
            let response_message = match client_msg {
                ClientMessageEnum::EchoMessage(echo) => {
                    info!("Received EchoMessage: {:?}", echo);
                    ServerMessageEnum::EchoMessage(EchoMessage { content: echo.content })
                }
                ClientMessageEnum::AddRequest(add) => {
                    let result = add.a + add.b;
                    ServerMessageEnum::AddResponse(AddResponse { result })
                }
            };

            info!("Sending response to client: {:?}", response_message);

            // Send the response back to the client
            self.send_response(ServerMessage { message: Some(response_message) })?;
        } else {
            warn!("Received a ClientMessage with no message variant.");
        }
    } else {
        error!("Failed to decode ClientMessage.");
    }

    Ok(())
    }

    fn send_response(&mut self, response: ServerMessage) -> io::Result<()> {
        let mut buf = Vec::new();
        response.encode(&mut buf)?;
        self.stream.write_all(&buf)?;
        Ok(())
    }
}

pub struct Server {
    listener: TcpListener,
    is_running: Arc<AtomicBool>,
}

impl Server {
    /// Creates a new server instance
    pub fn new(addr: &str) -> io::Result<Self> {
        let listener = TcpListener::bind(addr)?;
        let is_running = Arc::new(AtomicBool::new(false));
        Ok(Server {
            listener,
            is_running,
        })
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
        if self.is_running.load(Ordering::SeqCst) {
            self.is_running.store(false, Ordering::SeqCst);
            info!("Shutdown signal sent.");
        } else {
            warn!("Server was already stopped or not running.");
        }
    }
}
