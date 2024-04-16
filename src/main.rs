// Uncomment this block to pass the first stage

use std::borrow::Borrow;
use std::collections::HashMap;
use std::path::Path;

use anyhow::Context;
use clap::{Arg, Command};
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::net::tcp::ReadHalf;

#[derive(Debug)]
enum HttpStatusCode {
    Ok200,
    NotFound404,
}

#[derive(Debug)]
enum HttpMethod {
    Get,
}

#[derive(Debug)]
struct HttpRequest {
    method: HttpMethod,
    route: String,
    version: String,
    headers: HashMap<String, String>,
    content: Option<String>,
}

impl HttpRequest {
    fn response_404(&self) -> HttpResponseBuilder {
        HttpResponseBuilder {
            status_code: HttpStatusCode::NotFound404,
            version: self.version.clone(),
            content: None,
        }
    }

    fn response(&self, content: Option<String>) -> HttpResponseBuilder {
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
    content: Option<String>,
}

impl Into<String> for HttpResponseBuilder {
    fn into(self) -> String {
        let (code, phrase) = match self.status_code {
            HttpStatusCode::Ok200 => (200, "OK"),
            HttpStatusCode::NotFound404 => (404, "NotFound"),
        };
        let mut response = format!("{} {} {}\r\n", self.version, code, phrase);
        if let Some(content) = self.content {
            response.push_str(&format!("Content-Type: text/plain\r\n"));
            response.push_str(&format!("Content-Length: {}\r\n", content.len()));
            response.push_str("\r\n");
            response.push_str(&content);
        } else {
            response.push_str("\r\n");
        }
        response
    }
}

async fn reader_request(reader: &mut BufReader<&mut ReadHalf<'_>>) -> anyhow::Result<HttpRequest> {
    let mut request_content = String::new();

    let prev = 0;
    while let n = reader.read_line(&mut request_content).await? {
        if n - prev == 2 {
            break;
        }
    }

    println!("DEBUG: content {request_content}");
    let mut lines = request_content.lines();

    // first line
    let first = lines
        .next()
        .context("ERROR reading request: empty request")?;
    let mut split = first.split(" ");
    let _ = split
        .next()
        .context("ERROR reading request: missing method")?;
    let method = HttpMethod::Get;
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
    let mut content = None;
    if let Some(length) = headers.get("Content-Length") {
        let length = length.parse()?;
        let mut buffer = Vec::with_capacity(length);
        reader.read_exact(&mut buffer).await?;
        content = Option::from(String::from_utf8(buffer)?);
    }

    Ok(HttpRequest {
        method,
        headers,
        content,
        route: route.to_string(),
        version: version.to_string(),
    })
}

async fn route_request(request: HttpRequest, directory: String) -> HttpResponseBuilder {
    let route = request.route.split('/').skip(1).collect::<Vec<&str>>();
    println!("DEBUG: route {route:?}");
    let response = match route.as_slice() {
        [""] => HttpResponseBuilder {
            status_code: HttpStatusCode::Ok200,
            version: request.version,
            content: None,
        },
        ["echo", val @ ..] => {
            request.response(Some(val.join("/").to_string()))
        }
        ["user-agent"] => {
            let user_agent = request.headers.get("User-Agent");
            match user_agent {
                Some(user_agent) => {
                    let user_agent = user_agent.clone();
                    request.response(Some(user_agent))
                }
                None => {
                    request.response_404()
                }
            }
        }
        ["files", filename] => {
            let dir = Path::new(&directory);
            let file_path = dir.join(filename);
            println!("DEBUG: {}", file_path.display());

            let mut file = match File::open(&file_path).await {
                Err(err) => {
                    eprintln!("ERROR: couldn't open path {}, error: {err}", file_path.display());
                    return request.response_404();
                }
                Ok(file) => file
            };

            let mut file_content = String::new();
            match file.read_to_string(&mut file_content).await {
                Ok(_) => {
                    request.response(Some(file_content))
                }
                Err(err) => {
                    eprintln!("ERROR: couldn't read file, error: {err}");
                    request.response_404()
                }
            }
        }

        _ => request.response_404(),
    };
    response
}

async fn stream_handler(mut stream: TcpStream, directory: String) -> anyhow::Result<()> {
    let (mut reader, mut writer) = stream.split();
    let mut reader = BufReader::new(&mut reader);
    let request = reader_request(&mut reader).await?;
    println!("DEBUG: request {:?}", request);

    let response = route_request(request, directory).await;
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
                .required(true)
        )
        .get_matches();

    let directory = matches
        .get_one::<String>("directory")
        .unwrap()
        .to_string();
    println!("DEBUG: directory {:?}", directory);


    let addr = "127.0.0.1:4221";
    let listener = TcpListener::bind(addr).await?;
    println!("INFO: listening {addr}");

    loop {
        let (stream, _) = listener.accept().await?;
        let directory = directory.clone();
        tokio::spawn(
            async move {
                if let Err(err) = stream_handler(stream, directory).await {
                    eprintln!("ERROR: connection ended with {err}")
                }
            }
        );
    }
}
