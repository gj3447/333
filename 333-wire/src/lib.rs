// KG: SPAN_333_Wire, queue-network-stress-cross-node-2026-04-18
//
// 333 P2P OS wire protocol — the minimum needed to let two nodes run consensus
// over real TCP sockets. No tokio, no serde: hand-rolled length-prefixed
// frames so cross-crate edge cases are visible in the codec itself.
//
// Frame layout:
//   [4 bytes big-endian: body_len N]
//   [1 byte:             msg_type]
//   [N-1 bytes:          body]
//
// msg_type:
//   0x01 Proposal  — block bytes
//   0x02 Vote      — vote bytes
//   0x03 Envelope  — signaling envelope bytes
//
// Body encodings mirror the existing `canonical_bytes` conventions so the same
// byte view reaches every node that sees the frame.

#![forbid(unsafe_code)]

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use consensus333::{
    Block, BlockFinality, ConsensusProtocol, InMemoryConsensus, SettlementOp, ValidatorSet, Vote,
    VoteKind,
};
use identity333::{Keypair, NodeId, Signature};
use signaling333::{Envelope, Topic};
use thiserror::Error;

pub const MSG_PROPOSAL: u8 = 0x01;
pub const MSG_VOTE: u8 = 0x02;
pub const MSG_ENVELOPE: u8 = 0x03;

// ============================================================================
// Errors
// ============================================================================

#[derive(Debug, Error)]
pub enum WireError {
    #[error("io: {0}")]
    Io(#[from] io::Error),
    #[error("short frame: need {need} got {got}")]
    Short { need: usize, got: usize },
    #[error("unknown msg type: 0x{0:02x}")]
    UnknownType(u8),
    #[error("malformed body: {0}")]
    Malformed(&'static str),
    #[error("peer lost")]
    PeerLost,
}

// ============================================================================
// Frame
// ============================================================================

#[derive(Debug, Clone)]
pub enum Frame {
    Proposal(Block),
    Vote(Vote),
    Envelope(Envelope),
}

// ---- Block codec ----------------------------------------------------------

fn encode_block(b: &Block) -> Vec<u8> {
    let mut out = Vec::with_capacity(128 + b.ops.len() * 96);
    out.extend_from_slice(&b.height.to_be_bytes());
    out.extend_from_slice(b.proposer.as_bytes());
    out.extend_from_slice(&b.parent_hash);
    out.push(match b.finality {
        BlockFinality::Tentative => 0,
        BlockFinality::Confirmed => 1,
        BlockFinality::Committed => 2,
    });
    out.extend_from_slice(&(b.ops.len() as u32).to_be_bytes());
    for op in &b.ops {
        encode_op(op, &mut out);
    }
    out
}

fn encode_op(op: &SettlementOp, out: &mut Vec<u8>) {
    match op {
        SettlementOp::Transfer { from, to, amount, payload } => {
            out.push(1);
            out.extend_from_slice(from.as_bytes());
            out.extend_from_slice(to.as_bytes());
            out.extend_from_slice(&amount.to_be_bytes());
            out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
            out.extend_from_slice(payload);
        }
        SettlementOp::AuctionBid { bidder, item, amount } => {
            out.push(2);
            out.extend_from_slice(bidder.as_bytes());
            out.extend_from_slice(&(item.len() as u32).to_be_bytes());
            out.extend_from_slice(item.as_bytes());
            out.extend_from_slice(&amount.to_be_bytes());
        }
        SettlementOp::RankedAction { actor, kind, rank, payload } => {
            out.push(3);
            out.extend_from_slice(actor.as_bytes());
            out.extend_from_slice(&(kind.len() as u32).to_be_bytes());
            out.extend_from_slice(kind.as_bytes());
            out.extend_from_slice(&rank.to_be_bytes());
            out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
            out.extend_from_slice(payload);
        }
    }
}

struct Cur<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cur<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], WireError> {
        if self.pos + n > self.buf.len() {
            return Err(WireError::Short { need: self.pos + n, got: self.buf.len() });
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    fn u32be(&mut self) -> Result<u32, WireError> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64be(&mut self) -> Result<u64, WireError> {
        Ok(u64::from_be_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn u8(&mut self) -> Result<u8, WireError> {
        Ok(self.take(1)?[0])
    }
    fn node_id(&mut self) -> Result<NodeId, WireError> {
        let b: [u8; 32] = self.take(32)?.try_into().unwrap();
        Ok(NodeId::from_bytes(b))
    }
    fn sig(&mut self) -> Result<Signature, WireError> {
        let b: [u8; 64] = self.take(64)?.try_into().unwrap();
        Ok(Signature(b))
    }
    fn varbytes(&mut self) -> Result<Vec<u8>, WireError> {
        let n = self.u32be()? as usize;
        Ok(self.take(n)?.to_vec())
    }
    fn varstr(&mut self) -> Result<String, WireError> {
        let bytes = self.varbytes()?;
        String::from_utf8(bytes).map_err(|_| WireError::Malformed("invalid utf8"))
    }
}

fn decode_block(buf: &[u8]) -> Result<Block, WireError> {
    let mut c = Cur::new(buf);
    let height = c.u64be()?;
    let proposer = c.node_id()?;
    let parent_bytes: [u8; 32] = c.take(32)?.try_into().unwrap();
    let fin = match c.u8()? {
        0 => BlockFinality::Tentative,
        1 => BlockFinality::Confirmed,
        2 => BlockFinality::Committed,
        _ => return Err(WireError::Malformed("bad finality tag")),
    };
    let n_ops = c.u32be()? as usize;
    let mut ops = Vec::with_capacity(n_ops);
    for _ in 0..n_ops {
        let tag = c.u8()?;
        let op = match tag {
            1 => {
                let from = c.node_id()?;
                let to = c.node_id()?;
                let amount = c.u64be()?;
                let payload = c.varbytes()?;
                SettlementOp::Transfer { from, to, amount, payload }
            }
            2 => {
                let bidder = c.node_id()?;
                let item = c.varstr()?;
                let amount = c.u64be()?;
                SettlementOp::AuctionBid { bidder, item, amount }
            }
            3 => {
                let actor = c.node_id()?;
                let kind = c.varstr()?;
                let rank = c.u32be()?;
                let payload = c.varbytes()?;
                SettlementOp::RankedAction { actor, kind, rank, payload }
            }
            _ => return Err(WireError::Malformed("bad op tag")),
        };
        ops.push(op);
    }
    Ok(Block {
        height,
        proposer,
        parent_hash: parent_bytes,
        ops,
        finality: fin,
    })
}

// ---- Vote codec -----------------------------------------------------------

fn encode_vote(v: &Vote) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + 32 + 32 + 1 + 64);
    out.extend_from_slice(&v.height.to_be_bytes());
    out.extend_from_slice(&v.block_hash);
    out.extend_from_slice(v.voter.as_bytes());
    out.push(match v.kind {
        VoteKind::Prevote => 1,
        VoteKind::Precommit => 2,
    });
    out.extend_from_slice(&v.sig.0);
    out
}

fn decode_vote(buf: &[u8]) -> Result<Vote, WireError> {
    let mut c = Cur::new(buf);
    let height = c.u64be()?;
    let bh: [u8; 32] = c.take(32)?.try_into().unwrap();
    let voter = c.node_id()?;
    let kind = match c.u8()? {
        1 => VoteKind::Prevote,
        2 => VoteKind::Precommit,
        _ => return Err(WireError::Malformed("bad vote kind")),
    };
    let sig = c.sig()?;
    Ok(Vote { height, block_hash: bh, voter, kind, sig })
}

// ---- Envelope codec -------------------------------------------------------

fn encode_envelope(e: &Envelope) -> Vec<u8> {
    let mut out = Vec::with_capacity(128 + e.payload.len());
    let topic = e.topic.as_str();
    out.extend_from_slice(&(topic.len() as u32).to_be_bytes());
    out.extend_from_slice(topic.as_bytes());
    out.extend_from_slice(e.from.as_bytes());
    match &e.to {
        Some(n) => {
            out.push(1);
            out.extend_from_slice(n.as_bytes());
        }
        None => out.push(0),
    }
    out.extend_from_slice(&(e.payload.len() as u32).to_be_bytes());
    out.extend_from_slice(&e.payload);
    out.extend_from_slice(&e.seq.to_be_bytes());
    out.extend_from_slice(&e.sig.0);
    out
}

fn decode_envelope(buf: &[u8]) -> Result<Envelope, WireError> {
    let mut c = Cur::new(buf);
    let topic = Topic::from_str(&c.varstr()?);
    let from = c.node_id()?;
    let has_to = c.u8()?;
    let to = if has_to == 1 {
        Some(c.node_id()?)
    } else if has_to == 0 {
        None
    } else {
        return Err(WireError::Malformed("bad to flag"));
    };
    let payload = c.varbytes()?;
    let seq = c.u64be()?;
    let sig = c.sig()?;
    Ok(Envelope { topic, from, to, payload, seq, sig })
}

// ---- Frame codec ----------------------------------------------------------

pub fn encode_frame(frame: &Frame) -> Vec<u8> {
    let (tag, body) = match frame {
        Frame::Proposal(b) => (MSG_PROPOSAL, encode_block(b)),
        Frame::Vote(v) => (MSG_VOTE, encode_vote(v)),
        Frame::Envelope(e) => (MSG_ENVELOPE, encode_envelope(e)),
    };
    let body_len = (body.len() + 1) as u32;
    let mut out = Vec::with_capacity(4 + body_len as usize);
    out.extend_from_slice(&body_len.to_be_bytes());
    out.push(tag);
    out.extend_from_slice(&body);
    out
}

pub fn decode_frame(bytes: &[u8]) -> Result<Frame, WireError> {
    if bytes.len() < 5 {
        return Err(WireError::Short { need: 5, got: bytes.len() });
    }
    let body_len = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    if bytes.len() < 4 + body_len {
        return Err(WireError::Short { need: 4 + body_len, got: bytes.len() });
    }
    let tag = bytes[4];
    let body = &bytes[5..4 + body_len];
    match tag {
        MSG_PROPOSAL => Ok(Frame::Proposal(decode_block(body)?)),
        MSG_VOTE => Ok(Frame::Vote(decode_vote(body)?)),
        MSG_ENVELOPE => Ok(Frame::Envelope(decode_envelope(body)?)),
        x => Err(WireError::UnknownType(x)),
    }
}

/// Read exactly one frame off `stream`. Blocks until a full frame arrives.
pub fn read_frame<R: Read>(stream: &mut R) -> Result<Frame, WireError> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf)?;
    let body_len = u32::from_be_bytes(len_buf) as usize;
    let mut body = vec![0u8; body_len];
    stream.read_exact(&mut body)?;
    let tag = body[0];
    let body = &body[1..];
    match tag {
        MSG_PROPOSAL => Ok(Frame::Proposal(decode_block(body)?)),
        MSG_VOTE => Ok(Frame::Vote(decode_vote(body)?)),
        MSG_ENVELOPE => Ok(Frame::Envelope(decode_envelope(body)?)),
        x => Err(WireError::UnknownType(x)),
    }
}

pub fn write_frame<W: Write>(stream: &mut W, frame: &Frame) -> Result<(), WireError> {
    let bytes = encode_frame(frame);
    stream.write_all(&bytes)?;
    stream.flush()?;
    Ok(())
}

// ============================================================================
// Node — single-process representation of one validator on the wire
// ============================================================================

pub struct Node {
    pub keypair: Keypair,
    pub consensus: Arc<InMemoryConsensus>,
    pub inbox_rx: Mutex<Option<Receiver<Frame>>>,
    peers: Mutex<Vec<TcpStream>>,
    listener_handle: Mutex<Option<JoinHandle<()>>>,
    bound: String,
}

impl Node {
    pub fn bind(
        keypair: Keypair,
        validators: ValidatorSet,
        addr: &str,
    ) -> Result<Arc<Self>, WireError> {
        let listener = TcpListener::bind(addr)?;
        let bound = listener.local_addr()?.to_string();
        let consensus = Arc::new(InMemoryConsensus::new(validators));
        let (tx, rx) = mpsc::channel::<Frame>();
        let node = Arc::new(Self {
            keypair,
            consensus,
            inbox_rx: Mutex::new(Some(rx)),
            peers: Mutex::new(Vec::new()),
            listener_handle: Mutex::new(None),
            bound,
        });
        let handle = Self::spawn_listener(listener, tx);
        *node.listener_handle.lock().unwrap() = Some(handle);
        Ok(node)
    }

    fn spawn_listener(listener: TcpListener, tx: Sender<Frame>) -> JoinHandle<()> {
        thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(stream) = conn else { continue };
                let tx = tx.clone();
                thread::spawn(move || {
                    let mut stream = stream;
                    loop {
                        match read_frame(&mut stream) {
                            Ok(frame) => {
                                if tx.send(frame).is_err() {
                                    break;
                                }
                            }
                            Err(_) => break,
                        }
                    }
                });
            }
        })
    }

    pub fn bound_addr(&self) -> &str {
        &self.bound
    }

    pub fn dial<A: ToSocketAddrs>(&self, addr: A) -> Result<(), WireError> {
        let stream = TcpStream::connect(addr)?;
        self.peers.lock().unwrap().push(stream);
        Ok(())
    }

    pub fn broadcast(&self, frame: &Frame) -> Result<(), WireError> {
        let mut peers = self.peers.lock().unwrap();
        for p in peers.iter_mut() {
            write_frame(p, frame)?;
        }
        Ok(())
    }

    /// Apply a frame to local consensus. Returns what changed.
    pub fn apply(&self, frame: &Frame) -> Result<(), WireError> {
        match frame {
            Frame::Proposal(b) => {
                let _ = self.consensus.propose(b.clone());
            }
            Frame::Vote(v) => {
                let _ = self.consensus.vote(v.clone());
            }
            Frame::Envelope(_) => {
                // Envelope routing is a downstream concern; dropped in this smoke.
            }
        }
        Ok(())
    }

    /// Block until `n` frames have arrived on the inbox, applying each.
    pub fn drain_and_apply(&self, n: usize, deadline: std::time::Duration) -> usize {
        let start = std::time::Instant::now();
        let mut count = 0;
        let rx_guard = self.inbox_rx.lock().unwrap();
        let rx = rx_guard.as_ref().expect("inbox taken");
        while count < n {
            let remaining = deadline.saturating_sub(start.elapsed());
            if remaining.is_zero() {
                break;
            }
            match rx.recv_timeout(remaining) {
                Ok(frame) => {
                    let _ = self.apply(&frame);
                    count += 1;
                }
                Err(_) => break,
            }
        }
        count
    }
}

// ============================================================================
// Unit tests (codec)
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_block() -> Block {
        let kp = Keypair::generate();
        Block {
            height: 42,
            proposer: kp.node_id(),
            parent_hash: [0xab; 32],
            ops: vec![
                SettlementOp::Transfer {
                    from: Keypair::generate().node_id(),
                    to: Keypair::generate().node_id(),
                    amount: 1000,
                    payload: b"memo".to_vec(),
                },
                SettlementOp::RankedAction {
                    actor: kp.node_id(),
                    kind: "voxel.place".into(),
                    rank: 7,
                    payload: vec![1, 2, 3],
                },
            ],
            finality: BlockFinality::Tentative,
        }
    }

    #[test]
    fn frame_proposal_roundtrip() {
        let b = sample_block();
        let encoded = encode_frame(&Frame::Proposal(b.clone()));
        let decoded = decode_frame(&encoded).unwrap();
        if let Frame::Proposal(b2) = decoded {
            assert_eq!(b.height, b2.height);
            assert_eq!(b.parent_hash, b2.parent_hash);
            assert_eq!(b.ops.len(), b2.ops.len());
            assert_eq!(b.hash(), b2.hash(), "round-tripped block must hash identically");
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn frame_vote_roundtrip() {
        let kp = Keypair::generate();
        let h = [0x33; 32];
        let v = InMemoryConsensus::sign_vote(&kp, 7, h, VoteKind::Precommit);
        let encoded = encode_frame(&Frame::Vote(v.clone()));
        let decoded = decode_frame(&encoded).unwrap();
        if let Frame::Vote(v2) = decoded {
            assert_eq!(v.height, v2.height);
            assert_eq!(v.block_hash, v2.block_hash);
            assert_eq!(v.voter, v2.voter);
            assert_eq!(v.kind, v2.kind);
            assert_eq!(v.sig.0, v2.sig.0);
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn frame_envelope_roundtrip() {
        let kp = Keypair::generate();
        let env = Envelope::sign(
            &kp,
            Topic::Custom("/voxel/join".into()),
            Some(Keypair::generate().node_id()),
            b"payload".to_vec(),
            9,
        );
        let encoded = encode_frame(&Frame::Envelope(env.clone()));
        let decoded = decode_frame(&encoded).unwrap();
        if let Frame::Envelope(e2) = decoded {
            assert_eq!(env.topic.as_str(), e2.topic.as_str());
            assert_eq!(env.from, e2.from);
            assert_eq!(env.to, e2.to);
            assert_eq!(env.payload, e2.payload);
            assert_eq!(env.seq, e2.seq);
            e2.verify().unwrap(); // signature still valid after round-trip
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn unknown_type_rejected() {
        let mut buf = vec![0, 0, 0, 1, 0xff]; // length 1, unknown tag
        // pad body
        buf.push(0);
        buf[3] = 2;
        let err = decode_frame(&buf).unwrap_err();
        assert!(matches!(err, WireError::UnknownType(0xff)));
    }

    #[test]
    fn short_frame_detected() {
        let buf = [0, 0, 0, 100]; // claims 100 bytes, has none
        let err = decode_frame(&buf).unwrap_err();
        assert!(matches!(err, WireError::Short { .. }));
    }

    #[test]
    fn proposal_block_hash_stable_after_wire() {
        // Any two encoders/decoders must agree on block.hash() after round-trip.
        let b = sample_block();
        let h1 = b.hash();
        let enc = encode_frame(&Frame::Proposal(b));
        let dec = decode_frame(&enc).unwrap();
        if let Frame::Proposal(b2) = dec {
            assert_eq!(h1, b2.hash());
        }
    }
}
