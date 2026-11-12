//! A small lightwalletd stand-in serving a [`MockChain`] as gRPC over
//! HTTP/2 on loopback, for tests of the gRPC client and of a node that is
//! given `--zec-indexer`.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use bytes::{BufMut, Bytes, BytesMut};
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::server::conn::http2;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::{TokioExecutor, TokioIo};
use prost::Message;
use tokio::net::TcpListener;
use zcash_client_backend::proto::service::{BlockId, BlockRange, LightdInfo};

use super::MockChain;
use crate::ZecChain;

/// What the stand-in serves, and every method it was asked for.
pub struct Indexer {
    pub chain: Arc<MockChain>,
    pub info: LightdInfo,
    pub calls: Mutex<Vec<String>>,
}

/// What a testnet server speaking lightwallet-protocol `protocol` says about itself.
pub fn testnet_info(protocol: &str) -> LightdInfo {
    LightdInfo {
        vendor: "Stand-in".into(),
        version: "0.0.1".into(),
        chain_name: "test".into(),
        sapling_activation_height: 280_000,
        lightwallet_protocol_version: protocol.into(),
        ..Default::default()
    }
}

/// gRPC framing: a zero flag byte, a big-endian length, the message.
fn framed(messages: &[Vec<u8>]) -> Bytes {
    let mut out = BytesMut::new();
    for message in messages {
        out.put_u8(0);
        out.put_u32(message.len() as u32);
        out.put_slice(message);
    }
    out.freeze()
}

async fn answer(indexer: Arc<Indexer>, request: Request<Incoming>) -> Result<Response<impl hyper::body::Body<Data = Bytes, Error = Infallible>>, Infallible> {
    let method = request.uri().path().rsplit('/').next().unwrap_or_default().to_string();
    let body = request.into_body().collect().await.map(|body| body.to_bytes()).unwrap_or_default();
    let message = body.get(5..).unwrap_or_default();
    indexer.calls.lock().unwrap().push(method.clone());
    let chain = &indexer.chain;
    let result: Result<Vec<Vec<u8>>, String> = match method.as_str() {
        "GetLightdInfo" => Ok(vec![indexer.info.encode_to_vec()]),
        "GetLatestBlock" => chain.tip().and_then(|tip| Ok(vec![BlockId { height: tip, hash: chain.block_hash(tip)?.to_vec() }.encode_to_vec()])),
        "GetBlock" => {
            let id = BlockId::decode(message).unwrap();
            chain.blocks(id.height, id.height).map(|blocks| vec![blocks[0].encode_to_vec()])
        }
        "GetBlockRange" => {
            let range = BlockRange::decode(message).unwrap();
            assert!(range.pool_types.is_empty(), "no pool filter: Ironwood is in the default");
            chain.blocks(range.start.unwrap().height, range.end.unwrap().height).map(|blocks| blocks.iter().map(Message::encode_to_vec).collect())
        }
        other => Err(format!("{other} is not served")),
    };
    let mut trailers = http::HeaderMap::new();
    let data = match result {
        Ok(messages) => {
            trailers.insert("grpc-status", "0".parse().unwrap());
            framed(&messages)
        }
        Err(message) => {
            trailers.insert("grpc-status", "5".parse().unwrap());
            trailers.insert("grpc-message", message.replace(' ', "%20").parse().unwrap());
            Bytes::new()
        }
    };
    let body = Full::new(data).with_trailers(std::future::ready(Some(Ok(trailers))));
    Ok(Response::builder().header("content-type", "application/grpc").body(body).unwrap())
}

/// Serve `indexer` on a loopback port of `runtime`. Returns the address.
pub fn serve(runtime: &tokio::runtime::Runtime, indexer: Arc<Indexer>) -> SocketAddr {
    let listener = runtime.block_on(TcpListener::bind("127.0.0.1:0")).unwrap();
    let address = listener.local_addr().unwrap();
    runtime.spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else { return };
            let indexer = Arc::clone(&indexer);
            tokio::spawn(async move {
                let service = service_fn(move |request| answer(Arc::clone(&indexer), request));
                let _ = http2::Builder::new(TokioExecutor::new()).serve_connection(TokioIo::new(stream), service).await;
            });
        }
    });
    address
}

/// A testnet stand-in for `chain` that says it speaks `protocol`.
pub fn indexer(chain: &Arc<MockChain>, protocol: &str) -> Arc<Indexer> {
    Arc::new(Indexer { chain: Arc::clone(chain), info: testnet_info(protocol), calls: Mutex::new(Vec::new()) })
}
