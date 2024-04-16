// Uncomment this block to pass the first stage

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

async fn handler(mut stream: TcpStream) -> anyhow::Result<()> {
    let (mut reader, mut writer) = stream.split();
    let mut reader = BufReader::new(reader);
    let mut x = String::new();
    let z = reader.read_line(&mut x).await?; // HTTP/1.1 200 OK\r\n
    println!("{}", z);

    x.clear();
    let z = reader.read_line(&mut x).await?; // \r\n
    println!("{}", z);

    writer.write_all(
        format!("HTTP/1.1 200 OK\r\n\r\n").as_bytes()
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
