#![allow(dead_code)]
use std::{
    collections::HashMap,
    error::Error,
    io::{ErrorKind, Read, Write},
};

use mio::net::{TcpListener, TcpStream};
use mio::{Events, Interest, Poll, Token};
use serde_json::json;

use crate::stats::program_information;

// Some tokens to allow us to identify which event is for which socket.
const SERVER: Token = Token(0);

struct RequestContext {
    connection: TcpStream,
    buffer: Vec<u8>,
    to_write: Vec<u8>,
    offset: usize,
}

impl RequestContext {
    fn new(stream: TcpStream) -> Self {
        Self {
            connection: stream,
            buffer: Vec::with_capacity(1024),
            to_write: Vec::new(),
            offset: 0,
        }
    }

    fn fill_to_write(&mut self) {
        let info = program_information();
        let data: Vec<(usize, usize)> = info.memory_map.iter().map(|element| *element).collect();
        let data_json = json!({
            "memory": data,
            "length_memory_array": data.len(),
            "memory_allocated": info.memory_allocated,
            "total_memory": info.total_memory
        });
        let string = data_json.to_string();
        self.to_write = get_html(&string);
    }
}

pub fn get_html(data: &str) -> Vec<u8> {
    return format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        data.len(),
        data
    )
    .as_bytes()
    .to_vec();
}

///
/// Creates a server which always sends a json with the memory map of your program
/// ## Example
/// GET / (Any route)
/// ```
/// {"length_memory_array":13,"memory":[[140414371781232,5],[140414371781296,64],[140414371781408,48],[140414371781552,80],[140414371781840,24],[140414371782064,64],[140414375991296,1024],[140414371781376,24],[140414371781456,64],[140414375987200,1024],[140414372823168,308],[140414384374272,1024],[140414375992320,4096]],"total_memory_allocated":7849}
/// ```
/// This means that the program is is occupying 7849 bytes, probably in the OS it seems that your program is occupying
/// more because of the RAM paging and the overhead of saving all of the addresses that your program is using.
/// The interesting part is the memory map of the program for example.
///
pub fn create_server_of_memory_map() -> Result<(), Box<dyn Error>> {
    // Create a poll instance.
    let mut poll = Poll::new()?;
    // Create storage for events.
    let mut events = Events::with_capacity(128);
    // Unique id for a connection
    let mut id = 2;
    // Connections map
    let mut connections: HashMap<Token, RequestContext> = HashMap::new();
    // Setup the server socket.
    let addr = "127.0.0.1:8080".parse()?;
    // Server listener
    let mut server = TcpListener::bind(addr)?;
    // Start listening for incoming connections.
    poll.registry()
        .register(&mut server, SERVER, Interest::READABLE)?;
    loop {
        // Poll Mio for events, blocking until we get an event.
        poll.poll(&mut events, None)?;

        // Process each event.
        for event in events.iter() {
            // We can use the token we previously provided to `register` to
            // determine for which socket the event is.
            match event.token() {
                SERVER => {
                    println!("new connection...");
                    // If this is an event for the server, it means a connection
                    // is ready to be accepted.
                    let (mut stream, _addr) = server.accept()?;
                    // Register the connection to the server
                    poll.registry()
                        .register(&mut stream, Token(id), Interest::READABLE)?;
                    // Save this connection's stream relating it to the id
                    connections.insert(Token(id), RequestContext::new(stream));
                    id += 1;
                }
                Token(id) => {
                    // Read closed... ignore
                    if event.is_read_closed() {
                        println!("closing connection {}...", id);
                        connections.remove(&event.token());
                        continue;
                    }
                    // Get the connection from the hashmap
                    let mut connection = connections.get_mut(&event.token());
                    // If it doesn't exist just inform and remove
                    if let None = connection {
                        println!("unknown connection id: {}", id);
                        continue;
                    }
                    // Unwrap and get mutable ref
                    let connection = &mut connection.as_mut().unwrap();
                    if event.is_writable() {
                        let mut result = connection
                            .connection
                            .write(&connection.to_write[connection.offset..]);
                        let result = do_callbacks(
                            &mut result,
                            |res| {
                                let bytes_written = *res;
                                if bytes_written + connection.offset >= connection.to_write.len() {
                                    Ok(())
                                } else {
                                    // Don't delete just yet
                                    connection.offset += bytes_written;
                                    Err(())
                                }
                            },
                            |err| {
                                // would block so let's try writing again
                                if let ErrorKind::WouldBlock = err.kind() {
                                    Err(())
                                } else {
                                    // Just remove
                                    Ok(())
                                }
                            },
                        );
                        // If we should close connection
                        if result.is_ok() {
                            println!("closing connection with {:?}", event.token());
                            poll.registry().deregister(&mut connection.connection)?;
                            connections.remove(&event.token());
                        }
                    } else if event.is_readable() {
                        // 10023B sized buffer initialization
                        let mut buff = [0u8; 10023];
                        // Read result
                        let mut result = connection.connection.read(&mut buff);
                        // Handle different results with callback
                        // I know it's kinda trashy but I wanted to write
                        // something like this
                        let result = do_callbacks(
                            &mut result,
                            // on ok run this
                            |result| {
                                let written_bytes = *result;
                                // this is kinda dangerous, imagine that the request is 10023 * x sized!!
                                // but because this is a toy server, this implementation is just good enough;
                                // in a real case scenario we would be reading until we find the end of the headers,
                                // then check the Content-Length to see how many bytes are we expecting...
                                if written_bytes >= buff.len() {
                                    // keep reading
                                    connection.buffer.extend_from_slice(&buff);
                                } else {
                                    connection.fill_to_write();
                                    // let's start writing
                                    poll.registry()
                                        .deregister(&mut connection.connection)
                                        .map_err(|_| ())?;
                                    poll.registry()
                                        .register(
                                            &mut connection.connection,
                                            event.token(),
                                            Interest::WRITABLE,
                                        )
                                        .map_err(|_| ())?;
                                }
                                Ok(())
                            },
                            // Handle error
                            |err| {
                                // would block so let's try reading again
                                if let ErrorKind::WouldBlock = err.kind() {
                                    Ok(())
                                } else {
                                    Err(())
                                }
                            },
                        );
                        if result.is_err() {
                            connections.remove(&event.token());
                        }
                    }
                }
            }
        }
    }
}

/// Run those callbacks depending on the result
/// Passes the `Ok` `result` on the `if_ok`
/// callback and the `Err` `result` on the `if_err` callback
fn do_callbacks<'a, T: 'static, E: 'static, OF, EF>(
    mut result: &'a mut Result<T, E>,
    mut if_ok: OF,
    mut if_err: EF,
) -> Result<(), ()>
where
    OF: FnMut(&mut T) -> Result<(), ()>,
    EF: FnMut(&mut E) -> Result<(), ()>,
{
    if let Ok(result) = &mut result {
        if_ok(result)
    } else if let Err(err) = &mut result {
        if_err(err)
    } else {
        unreachable!()
    }
}
