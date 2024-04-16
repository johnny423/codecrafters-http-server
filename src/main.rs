use std::borrow::Borrow;
use std::collections::HashMap;
use std::path::Path;

use anyhow::Context;
use clap::{Arg, Command};
use nom::bytes::complete::{tag, take_while1};
use nom::character::complete::{char, crlf, space0};
use nom::IResult;
use nom::multi::many1;
use nom::sequence::{pair, terminated};
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
    body: Option<String>,
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

    // read until empty line
    let mut temp_line = String::new();
    while let Ok(n) = reader.read_line(&mut temp_line).await {
        if n == 0 || temp_line.trim().is_empty() {
            break;
        }
        request_content.push_str(&temp_line); // Append the non-empty line
        temp_line.clear();
    }

    println!("DEBUG: content {request_content}");

    // parse request
    let (_left, mut request) = parse_http_request(&request_content)
        .map_err(|e| e.to_owned())?;


    // read body
    let body = if let Some(length) = request.headers.get("Content-Length") {
        println!("here!!");
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
    } else {
        println!("there");
        None
    };

    request.body = body;
    Ok(request)
}

fn non_whitespace(input: &str) -> IResult<&str, &str> {
    take_while1(|c: char| !c.is_whitespace())(input)
}

fn parse_http_request(content: &str) -> IResult<&str, HttpRequest> {
    let (input, method) = terminated(non_whitespace, space0)(content)?;
    let (input, route) = terminated(non_whitespace, space0)(input)?;
    let (input, version) = terminated(non_whitespace, crlf)(input)?;

    let (input, headers) = many1(pair(
        terminated(take_while1(|c: char| c != ':'), tag(": ")),
        terminated(non_whitespace, crlf),
    ))(input)?;

    let method = match method {
        "GET" => HttpMethod::Get,
        "POST" => HttpMethod::Post,
        _ => { panic!(); }
    };

    let headers: HashMap<String, String> = headers.into_iter()
        .map(|(n, v)| (n.to_string(), v.to_string()))
        .collect();

    Ok(
        (input, HttpRequest {
            method,
            headers,
            route: route.to_string(),
            version: version.to_string(),
            body: None,
        }
        )
    )
}

async fn route_request(request: &HttpRequest, directory: Option<String>) -> anyhow::Result<HttpResponseBuilder> {
    let route = request.route.split('/').skip(1).collect::<Vec<&str>>();
    println!("DEBUG: route {route:?}");
    let response = match (&request.method, route.as_slice()) {
        (HttpMethod::Get, [""]) => {
            let content = Content::Empty;
            Ok(
                HttpResponseBuilder {
                    status_code: HttpStatusCode::Ok200,
                    version: request.version.clone(),
                    content,
                }
            )
        }
        (HttpMethod::Get, ["echo", val @ ..]) => {
            let content = Content::Text(val.join("/").to_string());
            Ok(
                HttpResponseBuilder {
                    status_code: HttpStatusCode::Ok200,
                    version: request.version.clone(),
                    content,
                }
            )
        }
        (HttpMethod::Get, ["user-agent"]) => {
            let user_agent = request.headers.get("User-Agent");
            match user_agent {
                Some(user_agent) => {
                    let user_agent = user_agent.clone();
                    let content = Content::Text(user_agent);
                    Ok(
                        HttpResponseBuilder {
                            status_code: HttpStatusCode::Ok200,
                            version: request.version.clone(),
                            content,
                        }
                    )
                }
                None => {
                    Ok(
                        HttpResponseBuilder {
                            status_code: HttpStatusCode::NotFound404,
                            version: request.version.clone(),
                            content: Content::Empty,
                        }
                    )
                }
            }
        }
        (HttpMethod::Get, ["files", filename]) => {
            let file_path = match directory {
                None => {
                    return Ok(HttpResponseBuilder {
                        status_code: HttpStatusCode::NotFound404,
                        version: request.version.clone(),
                        content: Content::Empty,
                    });
                }
                Some(directory) => {
                    let dir = Path::new(&directory);
                    dir.join(filename)
                }
            };

            println!("DEBUG: {}", file_path.display());

            let mut file = match File::open(&file_path).await {
                Err(err) => {
                    eprintln!("ERROR: couldn't open path {}, error: {err}", file_path.display());
                    return Ok(HttpResponseBuilder {
                        status_code: HttpStatusCode::NotFound404,
                        version: request.version.clone(),
                        content: Content::Empty,
                    });
                }
                Ok(file) => file
            };

            let mut file_content = String::new();
            match file.read_to_string(&mut file_content).await {
                Ok(_) => {
                    let content = Content::OctetStream(file_content);
                    Ok(
                        HttpResponseBuilder {
                            status_code: HttpStatusCode::Ok200,
                            version: request.version.clone(),
                            content,
                        }
                    )
                }
                Err(err) => {
                    eprintln!("ERROR: couldn't read file, error: {err}");
                    Ok(HttpResponseBuilder {
                        status_code: HttpStatusCode::NotFound404,
                        version: request.version.clone(),
                        content: Content::Empty,
                    })
                }
            }
        }
        (HttpMethod::Post, ["files", filename]) => {
            let content = request.body.clone().context("Error: got no content")?;
            let file_path = match directory {
                None => {
                    return Ok(HttpResponseBuilder {
                        status_code: HttpStatusCode::NotFound404,
                        version: request.version.clone(),
                        content: Content::Empty,
                    });
                }
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
        _ => Ok(HttpResponseBuilder {
            status_code: HttpStatusCode::NotFound404,
            version: request.version.clone(),
            content: Content::Empty,
        }),
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
