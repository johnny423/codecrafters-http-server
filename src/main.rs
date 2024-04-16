// Uncomment this block to pass the first stage

use std::collections::HashMap;

use anyhow::Context;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::net::tcp::ReadHalf;

#[derive(Debug)]
enum HttpMethod {
    Get
}

#[derive(Debug)]
struct HttpRequest {
    method: HttpMethod,
    route: String,
    version: String,
    headers: HashMap<String, String>,
}

async fn reader_request(reader: &mut BufReader<&mut ReadHalf<'_>>) -> anyhow::Result<HttpRequest> {
    let mut content = String::new();

    let prev = 0;
    while let n = reader.read_line(&mut content).await? {
        if n - prev == 2 {
            break;
        }
    };

    println!("DEBUG: content {content}");
    let mut lines = content.lines();

    // first line
    let first = lines.next().context("ERROR reading request: empty request")?; // method;
    let mut split = first.split(" ");
    let _ = split.next().context("ERROR reading request: missing method")?; // method
    let method = HttpMethod::Get;
    let route = split.next().context("ERROR reading request: missing route")?;
    let version = split.next().context("ERROR reading request: missing version")?;

    // headers
    let mut headers = HashMap::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        let (name, val) = line.split_once(":")
            .with_context(|| format!("ERROR reading request: bad header '{line}'"))?;

        headers.insert(
            name.to_string(), val.to_string());
    }

    Ok(
        HttpRequest {
            method,
            route: route.to_string(),
            version: version.to_string(),
            headers: headers,
        }
    )
}

async fn handler(mut stream: TcpStream) -> anyhow::Result<()> {
    let (mut reader, mut writer) = stream.split();
    let mut reader = BufReader::new(&mut reader);
    let request = reader_request(&mut reader).await?;
    println!("DEBUG: request {:?}", request);

    let response = match request.route.as_ref() {
        "/" => { 200 }
        _ => { 404 }
    };

    writer.write_all(
        format!("HTTP/1.1 {response} OK\r\n\r\n").as_bytes()
    ).await?;


    Ok(())
}


#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let addr = "127.0.0.1:4221";
    let listener = TcpListener::bind(addr).await?;
    println!("listening {addr}");

    loop {
        let (stream, _) = listener.accept().await?;
        if let Err(err) = handler(stream).await {
            eprintln!("ERROR: connection ended with {err}")
        }
    }
}
