use std::{fmt, net::SocketAddr, sync::Arc};

use anyhow::Result;
use dashmap::DashMap;
//use tokio::net::unix::SocketAddr;
use futures::{
    stream::{SplitStream, StreamExt},
    SinkExt,
};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::mpsc,
};
use tokio_util::codec::{Framed, LinesCodec};
use tracing::{info, warn};

#[derive(Debug)]
struct State {
    // define peers as a dashmap, the key is socket address and the value is a mpsc sender channel.
    peers: DashMap<SocketAddr, mpsc::Sender<Arc<Message>>>,
}

#[derive(Debug, Clone)]
struct Message {
    sender: String,
    content: String,
}

#[derive(Debug)]
struct Peer {
    username: String,
    stream: SplitStream<Framed<TcpStream, LinesCodec>>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // initialize trace subscriber
    tracing_subscriber::fmt::init();

    //initialize the address
    let addr = "0.0.0.0:8080";
    //initialize the listener and bind the address
    let listener = TcpListener::bind(addr).await?;
    //print the address
    info!("Listening on: {}", addr);
    // initialize the state
    let state = Arc::new(State::new());

    //loop to accept the incoming connections
    loop {
        let (socket, addr) = listener.accept().await?;
        //print the address of the incoming connection
        info!("Accepted connection from: {}", addr);
        //clone the state reference
        let state_clone = state.clone();
        //spawn the connection to a new task
        tokio::spawn(async move {
            if let Err(e) = handle_connection(state_clone, addr, socket).await {
                warn!("failed to handle connection: {:?}", e);
            }
        });
    }

    #[allow(unreachable_code)]
    Ok(())
}

async fn handle_connection(state: Arc<State>, addr: SocketAddr, socket: TcpStream) -> Result<()> {
    //initialize the framed stream
    let mut framed = Framed::new(socket, LinesCodec::new());
    //send the welcome message
    framed.send("Enter your username:").await?;
    //receive the username
    let username = match framed.next().await {
        Some(Ok(username)) => username,
        _ => {
            warn!("Failed to get username from peer: {:?}", addr);
            return Ok(());
        }
    };

    //send the welcome message
    framed.send(format!("Welcome, {}!", username)).await?;
    //send the message to all the peers
    state
        .broadcast(
            addr,
            Arc::new(Message {
                sender: "Server".to_string(),
                content: format!("{} has joined the chat.", username),
            }),
        )
        .await;

    //add the peer to the state
    let mut peer = state.add_peer(addr, username, framed).await;
    //loop to receive the messages from the peer
    while let Some(line) = peer.stream.next().await {
        let line = line.unwrap();
        //send the message to all the peers
        state
            .broadcast(
                addr,
                Arc::new(Message {
                    sender: peer.username.clone(),
                    content: line,
                }),
            )
            .await;
    }

    //when the connection is closed, notify all the peers
    state
        .broadcast(
            addr,
            Arc::new(Message {
                sender: "Server".to_string(),
                content: format!("{} has left the chat.", peer.username),
            }),
        )
        .await;

    Ok(())
}

impl fmt::Display for Message {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}: {}", self.sender, self.content)
    }
}

impl State {
    fn new() -> Self {
        Self {
            peers: DashMap::new(),
        }
    }

    async fn broadcast(&self, addr: SocketAddr, message: Arc<Message>) {
        for peer in self.peers.iter() {
            if peer.key() == &addr {
                continue;
            }

            if peer.value().send(message.clone()).await.is_err() {
                info!("Failed to send message to peer: {:?}", peer.key());
                // remove the peer
                self.peers.remove(peer.key());
            }
        }
    }

    async fn add_peer(
        &self,
        addr: SocketAddr,
        username: String,
        stream: Framed<TcpStream, LinesCodec>,
    ) -> Peer {
        let (tx, mut rx) = mpsc::channel(16);

        self.peers.insert(addr, tx);

        // split the stream to read and write
        let (mut sender, receiver) = stream.split();

        // receive messages from others, and send them to the peer side
        tokio::spawn(async move {
            while let Some(message) = rx.recv().await {
                if sender.send(message.to_string()).await.is_err() {
                    info!("Failed to send message to peer: {:?}", addr);
                    break;
                }
            }
        });

        // return peer
        Peer {
            username,
            stream: receiver,
        }
    }
}
