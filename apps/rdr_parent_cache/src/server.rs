use http::{HeaderMap, Method, StatusCode};
use rdr_common::WireProtocol;
use reqwest::blocking::Client;
use std::{
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex},
};

/// Perform the HTTP GET request and write the response back to the TCP stream.
fn process_request(
    locked_stream: Arc<Mutex<Arc<TcpStream>>>,
    req: rdr_common::Request,
    http_client: Arc<Client>,
) {
    eprintln!("Processing request: {}", req.url);
    let http_request = http_client
        .request(Method::GET, req.url.clone())
        .headers(req.headers.clone());
    let response = match http_request.send() {
        Ok(response) => rdr_common::Response {
            original_request: req,
            url: response.url().clone(),
            status: response.status(),
            headers: response.headers().clone(),
            data: response.bytes().unwrap().to_vec(),
        },
        Err(e) => {
            eprintln!("Failed to perform request: {e}");
            rdr_common::Response {
                url: req.url.clone(),
                status: StatusCode::INTERNAL_SERVER_ERROR,
                headers: HeaderMap::new(),
                data: Vec::new(),
                original_request: req,
            }
        }
    };
    eprintln!(
        "Got response with status {} for {}",
        response.status, response.url
    );
    let stream = locked_stream.lock().unwrap();
    if let Err(e) = response.serialize_to(&mut stream.as_ref()) {
        eprintln!("Failed to send response to peer: {e}");
    }
}

/// Continually read from the TCP stream and parse HTTP GET requests.
fn read_requests(stream: TcpStream, http_client: Arc<Client>) {
    let stream = Arc::new(stream);
    // write access to TcpStream must be protected by Mutex to ensure that
    // entire data object is written atomically
    let locked_writable_stream = Arc::new(Mutex::new(stream.clone()));
    loop {
        match rdr_common::Request::extract_from(&mut stream.as_ref()) {
            Ok(req) => {
                let writable_2 = locked_writable_stream.clone();
                let http_client_2 = http_client.clone();
                std::thread::spawn(move || {
                    process_request(writable_2, req, http_client_2);
                });
            }
            Err(e) => {
                eprintln!("Failed to read request from peer: {e}");
                break;
            }
        }
    }
}

/// Start listening for client cache TCP connections on the specified port.
pub fn serve(port: u16) -> ! {
    let client = Arc::new(Client::new());
    let listener = TcpListener::bind(format!("0.0.0.0:{port}")).expect("Failed to bind to port");

    loop {
        let (stream, _) = listener.accept().expect("Failed to accept connection");
        let client_2 = client.clone();
        std::thread::spawn(move || {
            read_requests(stream, client_2);
        });
    }
}
