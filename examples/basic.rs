//! Basic example - just start the server

use siphonophore::Server;

#[tokio::main]
async fn main() {
    println!("Siphonophore listening on 0.0.0.0:8080");
    Server::new().serve("0.0.0.0:8080").await.unwrap();
}
