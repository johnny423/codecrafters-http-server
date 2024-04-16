use std::borrow::Borrow;
use std::collections::HashMap;
use std::path::Path;

use anyhow::{anyhow, Context};
use clap::{Arg, Command};
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::net::tcp::ReadHalf;

enum Content {
    Empty,
    Text(String),
    OctetStream(String),
}

#[derive(Debug)]
enum HttpStatusCode {
    Ok200,
    Created201,
    NotFound404,
    InternalError500,
}

#[derive(Debug)]
enum HttpMethod {
    Get,
    Post,
}

#[derive(Debug)]
struct HttpRequest {
    method: HttpMethod,
    route: String,
    version: String,
    headers: HashMap<String, String>,
    body: Option<String>, //todo: rename
}

impl HttpRequest {
    fn response_404(&self) -> HttpResponseBuilder {
        HttpResponseBuilder {
            status_code: HttpStatusCode::NotFound404,
            version: self.version.clone(),
            content: Content::Empty,
        }
    }

    fn response(&self, content: Content) -> HttpResponseBuilder {
        HttpResponseBuilder {
            status_code: HttpStatusCode::Ok200,
            version: self.version.clone(),
            content,
        }
    }
}

struct HttpResponseBuilder {
    status_code: HttpStatusCode,
    version: String,
    content: Content,
}

impl Into<String> for HttpResponseBuilder {
    fn into(self) -> String {
        let (code, phrase) = match self.status_code {
            HttpStatusCode::Ok200 => (200, "Ok"),
            HttpStatusCode::Created201 => (201, "Created"),
            HttpStatusCode::NotFound404 => (404, "NotFound"),
            HttpStatusCode::InternalError500 => (500, "InternalError"),
        };
        let mut response = format!("{} {} {}\r\n", self.version, code, phrase);
        match self.content {
            Content::Empty => {
                response.push_str("\r\n");
            }
            Content::Text(content) => {
                response.push_str(&format!("Content-Type: text/plain\r\n"));
                response.push_str(&format!("Content-Length: {}\r\n", content.len()));
                response.push_str("\r\n");
                response.push_str(&content);
            }
            Content::OctetStream(content) => {
                response.push_str(&format!("Content-Type: application/octet-stream\r\n"));
                response.push_str(&format!("Content-Length: {}\r\n", content.len()));
                response.push_str("\r\n");
                response.push_str(&content);
            }
        }

        response
    }
}

async fn reader_request(reader: &mut BufReader<&mut ReadHalf<'_>>) -> anyhow::Result<HttpRequest> {
    let mut request_content = String::new();

    let mut temp_line = String::new();
    while let Ok(n) = reader.read_line(&mut temp_line).await {
        if n == 0 || temp_line.trim().is_empty() {
            break;
        }
        request_content.push_str(&temp_line); // Append the non-empty line
        temp_line.clear();
    }

    println!("DEBUG: content {request_content}");
    let mut lines = request_content.lines();

    // first line
    let first = lines
        .next()
        .context("ERROR reading request: empty request")?;
    let mut split = first.split(" ");

    let method = split
        .next()
        .context("ERROR reading request: missing method")?;
    let method = match method {
        "GET" => HttpMethod::Get,
        "POST" => HttpMethod::Post,
        _ => { Err(anyhow!("Error reading request: unsupported method"))? }
    };

    let route = split
        .next()
        .context("ERROR reading request: missing route")?;
    let version = split
        .next()
        .context("ERROR reading request: missing version")?;

    // headers
    let mut headers = HashMap::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        let (name, val) = line
            .split_once(": ")
            .with_context(|| format!("ERROR reading request: bad header '{line}'"))?;

        headers.insert(name.to_string(), val.to_string());
    }

    // content
    let body = if let Some(length) = headers.get("Content-Length") {
        let length = length.parse()
            .context("ERROR: content length is not a valid number")?;
        println!("DEBUG: content length - {length}");

        let mut buffer = vec![0; length];
        reader.read_exact(&mut buffer).await
            .context("ERROR: reading request content")?;
        println!("DEBUG: buffer {buffer:?}");

        let x = String::from_utf8(buffer)
            .context("ERROR: request content is not utf8")?;
        println!("DEBUG: extracted content: {x}");
        Some(x)
    } else { None };

    Ok(HttpRequest {
        method,
        headers,
        body: body,
        route: route.to_string(),
        version: version.to_string(),
    })
}

async fn route_request(request: &HttpRequest, directory: Option<String>) -> anyhow::Result<HttpResponseBuilder> {
    let route = request.route.split('/').skip(1).collect::<Vec<&str>>();
    println!("DEBUG: route {route:?}");
    let response = match (&request.method, route.as_slice()) {
        (HttpMethod::Get, [""]) => {
            Ok(
                request.response(Content::Empty)
            )
        }
        (HttpMethod::Get, ["echo", val @ ..]) => {
            Ok(
                request.response(Content::Text(val.join("/").to_string()))
            )
        }
        (HttpMethod::Get, ["user-agent"]) => {
            let user_agent = request.headers.get("User-Agent");
            match user_agent {
                Some(user_agent) => {
                    let user_agent = user_agent.clone();
                    Ok(
                        request.response(Content::Text(user_agent))
                    )
                }
                None => {
                    Ok(
                        request.response_404()
                    )
                }
            }
        }
        (HttpMethod::Get, ["files", filename]) => {
            let file_path = match directory {
                None => { return Ok(request.response_404()); }
                Some(directory) => {
                    let dir = Path::new(&directory);
                    dir.join(filename)
                }
            };

            println!("DEBUG: {}", file_path.display());

            let mut file = match File::open(&file_path).await {
                Err(err) => {
                    eprintln!("ERROR: couldn't open path {}, error: {err}", file_path.display());
                    return Ok(request.response_404());
                }
                Ok(file) => file
            };

            let mut file_content = String::new();
            match file.read_to_string(&mut file_content).await {
                Ok(_) => {
                    Ok(
                        request.response(Content::OctetStream(file_content))
                    )
                }
                Err(err) => {
                    eprintln!("ERROR: couldn't read file, error: {err}");
                    Ok(request.response_404())
                }
            }
        }
        (HttpMethod::Post, ["files", filename]) => {
            let content = request.body.clone().context("Error: got no content")?;
            let file_path = match directory {
                None => { return Ok(request.response_404()); }
                Some(directory) => {
                    let dir = Path::new(&directory);
                    dir.join(filename)
                }
            };

            println!("DEBUG: {}", file_path.display());
            let mut file = File::create(&file_path).await?;
            file.write_all(content.as_bytes()).await?;
            Ok(
                HttpResponseBuilder {
                    status_code: HttpStatusCode::Created201,
                    version: request.version.clone(),
                    content: Content::Empty,
                }
            )
        }
        _ => Ok(request.response_404()),
    };
    response
}

async fn stream_handler(mut stream: TcpStream, directory: Option<String>) -> anyhow::Result<()> {
    let (mut reader, mut writer) = stream.split();
    let mut reader = BufReader::new(&mut reader);
    let request = reader_request(&mut reader).await?;
    println!("DEBUG: request {:?}", request);

    let response = route_request(&request, directory).await.unwrap_or_else(
        |_| HttpResponseBuilder {
            status_code: HttpStatusCode::InternalError500,
            version: request.version.clone(),
            content: Content::Empty,
        }
    );
    let response_string: String = response.into();

    println!("DEBUG: {response_string}");
    writer.write_all(response_string.as_bytes()).await?;

    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let matches = Command::new("http-server")
        .arg(
            Arg::new("directory")
                .long("directory")
                .required(false)
        )
        .get_matches();

    let directory = matches
        .get_one::<String>("directory");

    println!("DEBUG: directory {:?}", directory);


    let addr = "127.0.0.1:4221";
    let listener = TcpListener::bind(addr).await?;
    println!("INFO: listening {addr}");

    loop {
        let (stream, _) = listener.accept().await?;
        let directory = directory.cloned();
        tokio::spawn(
            async move {
                if let Err(err) = stream_handler(stream, directory).await {
                    eprintln!("ERROR: connection ended with {err}")
                }
            }
        );
    }
}
