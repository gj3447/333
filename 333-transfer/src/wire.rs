// KG: transport-plan Steps 1–3 (2026-07-14)
//
// Domain-separated, length-prefixed wire codec for authority-path messages.
// Mirrors `signing_message` style (domain tag + u64 LE length prefixes) so
// Transfer / Vote / Certificate can leave the process without reinventing QC.
//
// Real sockets are deferred (Steps 4–8); this module only owns the byte layout.

use ed25519_dalek::Signature;

use crate::authority::{Certificate, Vote};
use crate::Transfer;

const DOMAIN_TRANSFER: &[u8] = b"transfer333/wire-transfer/v1\0";
const DOMAIN_VOTE: &[u8] = b"transfer333/wire-vote/v1\0";
const DOMAIN_CERT: &[u8] = b"transfer333/wire-cert/v1\0";

/// Why a byte stream could not be decoded into an authority message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireError {
    Truncated,
    BadDomain,
    BadUtf8,
    BadSignature,
}

// --- low-level helpers -------------------------------------------------------

fn put_bytes(buf: &mut Vec<u8>, bytes: &[u8]) {
    buf.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    buf.extend_from_slice(bytes);
}

fn put_str(buf: &mut Vec<u8>, s: &str) {
    put_bytes(buf, s.as_bytes());
}

/// Transfer body without a domain tag (nested inside Vote / Certificate).
fn put_transfer_body(buf: &mut Vec<u8>, t: &Transfer) {
    put_str(buf, &t.from);
    buf.extend_from_slice(&t.from_seq.to_le_bytes());
    put_str(buf, &t.to);
    buf.extend_from_slice(&t.amount.to_le_bytes());
}

fn take<'a>(input: &mut &'a [u8], n: usize) -> Result<&'a [u8], WireError> {
    if input.len() < n {
        return Err(WireError::Truncated);
    }
    let (head, tail) = input.split_at(n);
    *input = tail;
    Ok(head)
}

fn take_u64(input: &mut &[u8]) -> Result<u64, WireError> {
    let b = take(input, 8)?;
    Ok(u64::from_le_bytes(b.try_into().expect("8 bytes")))
}

fn take_u128(input: &mut &[u8]) -> Result<u128, WireError> {
    let b = take(input, 16)?;
    Ok(u128::from_le_bytes(b.try_into().expect("16 bytes")))
}

fn take_len_bytes<'a>(input: &mut &'a [u8]) -> Result<&'a [u8], WireError> {
    let n = take_u64(input)? as usize;
    take(input, n)
}

fn take_str(input: &mut &[u8]) -> Result<String, WireError> {
    let b = take_len_bytes(input)?;
    std::str::from_utf8(b)
        .map(|s| s.to_owned())
        .map_err(|_| WireError::BadUtf8)
}

fn take_transfer_body(input: &mut &[u8]) -> Result<Transfer, WireError> {
    let from = take_str(input)?;
    let from_seq = take_u64(input)?;
    let to = take_str(input)?;
    let amount = take_u128(input)?;
    Ok(Transfer {
        from,
        from_seq,
        to,
        amount,
    })
}

fn take_signature(input: &mut &[u8]) -> Result<Signature, WireError> {
    let b = take(input, 64)?;
    let arr: [u8; 64] = b.try_into().map_err(|_| WireError::BadSignature)?;
    Ok(Signature::from_bytes(&arr))
}

fn expect_domain(input: &mut &[u8], domain: &[u8]) -> Result<(), WireError> {
    let got = take(input, domain.len())?;
    if got != domain {
        return Err(WireError::BadDomain);
    }
    Ok(())
}

fn put_vote_body(buf: &mut Vec<u8>, v: &Vote) {
    put_str(buf, &v.authority);
    put_transfer_body(buf, &v.transfer);
    buf.extend_from_slice(&v.signature.to_bytes());
}

fn take_vote_body(input: &mut &[u8]) -> Result<Vote, WireError> {
    let authority = take_str(input)?;
    let transfer = take_transfer_body(input)?;
    let signature = take_signature(input)?;
    Ok(Vote {
        authority,
        transfer,
        signature,
    })
}

// --- public encode / decode --------------------------------------------------

/// Encode a `Transfer` with domain separation.
pub fn encode_transfer(t: &Transfer) -> Vec<u8> {
    let mut buf = Vec::with_capacity(64 + t.from.len() + t.to.len());
    buf.extend_from_slice(DOMAIN_TRANSFER);
    put_transfer_body(&mut buf, t);
    buf
}

/// Decode a domain-tagged `Transfer`. Consumes the whole buffer (no trailing junk).
pub fn decode_transfer(bytes: &[u8]) -> Result<Transfer, WireError> {
    let mut input = bytes;
    expect_domain(&mut input, DOMAIN_TRANSFER)?;
    let t = take_transfer_body(&mut input)?;
    if !input.is_empty() {
        return Err(WireError::Truncated);
    }
    Ok(t)
}

/// Encode a signed `Vote` (authority id + transfer + 64-byte Ed25519 sig).
pub fn encode_vote(v: &Vote) -> Vec<u8> {
    let mut buf = Vec::with_capacity(128 + v.authority.len() + v.transfer.from.len() + v.transfer.to.len());
    buf.extend_from_slice(DOMAIN_VOTE);
    put_vote_body(&mut buf, v);
    buf
}

/// Decode a domain-tagged `Vote`.
pub fn decode_vote(bytes: &[u8]) -> Result<Vote, WireError> {
    let mut input = bytes;
    expect_domain(&mut input, DOMAIN_VOTE)?;
    let v = take_vote_body(&mut input)?;
    if !input.is_empty() {
        return Err(WireError::Truncated);
    }
    Ok(v)
}

/// Encode a quorum `Certificate` (transfer + full vote list).
pub fn encode_certificate(c: &Certificate) -> Vec<u8> {
    let mut buf = Vec::with_capacity(128 + c.votes.len() * 128);
    buf.extend_from_slice(DOMAIN_CERT);
    put_transfer_body(&mut buf, &c.transfer);
    buf.extend_from_slice(&(c.votes.len() as u64).to_le_bytes());
    for v in &c.votes {
        put_vote_body(&mut buf, v);
    }
    buf
}

/// Decode a domain-tagged `Certificate`. Validity is NOT checked here —
/// callers must still run `Certificate::is_valid` / `verify`.
pub fn decode_certificate(bytes: &[u8]) -> Result<Certificate, WireError> {
    let mut input = bytes;
    expect_domain(&mut input, DOMAIN_CERT)?;
    let transfer = take_transfer_body(&mut input)?;
    let n = take_u64(&mut input)? as usize;
    let mut votes = Vec::with_capacity(n);
    for _ in 0..n {
        votes.push(take_vote_body(&mut input)?);
    }
    if !input.is_empty() {
        return Err(WireError::Truncated);
    }
    Ok(Certificate { transfer, votes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authority::{signing_message, Authority, Certificate, Committee};
    use ed25519_dalek::{Signer, SigningKey};

    fn tx(from: &str, seq: u64, to: &str, amount: u128) -> Transfer {
        Transfer {
            from: from.into(),
            from_seq: seq,
            to: to.into(),
            amount,
        }
    }

    fn key(i: u8) -> SigningKey {
        SigningKey::from_bytes(&[i; 32])
    }

    fn setup(n: u8) -> (Committee, Vec<Authority>) {
        let auth: Vec<Authority> = (0..n)
            .map(|i| Authority::new(format!("a{i}"), key(i)))
            .collect();
        let committee =
            Committee::new(auth.iter().map(|a| (a.id().clone(), a.verifying_key()))).unwrap();
        (committee, auth)
    }

    #[test]
    fn transfer_round_trip() {
        let t = tx("alice", 7, "bob", 42);
        let bytes = encode_transfer(&t);
        assert_eq!(decode_transfer(&bytes).unwrap(), t);
    }

    #[test]
    fn vote_round_trip() {
        let t = tx("alice", 0, "bob", 10);
        let mut a = Authority::new("a0", key(0));
        let v = a.handle(&t).unwrap();
        let bytes = encode_vote(&v);
        let back = decode_vote(&bytes).unwrap();
        assert_eq!(back, v);
    }

    #[test]
    fn certificate_round_trip() {
        let t = tx("alice", 0, "bob", 30);
        let (c, mut auth) = setup(4);
        let votes: Vec<Vote> = auth
            .iter_mut()
            .take(3)
            .map(|a| a.handle(&t).unwrap())
            .collect();
        let cert = Certificate::assemble(t, votes, &c).expect("quorum");
        let bytes = encode_certificate(&cert);
        let back = decode_certificate(&bytes).unwrap();
        assert_eq!(back.transfer, cert.transfer);
        assert_eq!(back.votes, cert.votes);
        assert!(back.is_valid(&c));
    }

    #[test]
    fn wrong_domain_rejected() {
        let t = tx("alice", 0, "bob", 1);
        let mut bytes = encode_transfer(&t);
        // Corrupt the domain tag.
        bytes[0] ^= 0xff;
        assert_eq!(decode_transfer(&bytes), Err(WireError::BadDomain));
    }

    #[test]
    fn truncated_stream_rejected() {
        let t = tx("alice", 0, "bob", 1);
        let bytes = encode_transfer(&t);
        assert!(decode_transfer(&bytes[..bytes.len().saturating_sub(1)]).is_err());
    }

    #[test]
    fn tampered_vote_bytes_fail_certificate_is_valid() {
        // A flipped signature byte must not silently produce a valid cert.
        let t = tx("alice", 0, "bob", 10);
        let (committee, mut auth) = setup(4);
        let votes: Vec<Vote> = auth
            .iter_mut()
            .take(3)
            .map(|a| a.handle(&t).unwrap())
            .collect();
        let cert = Certificate::assemble(t.clone(), votes, &committee).unwrap();
        assert!(cert.is_valid(&committee));

        let mut bytes = encode_certificate(&cert);
        // Flip a byte in the trailing signature region (last 64 bytes of stream).
        let idx = bytes.len() - 1;
        bytes[idx] ^= 0x01;

        match decode_certificate(&bytes) {
            Ok(tampered) => {
                assert!(
                    !tampered.is_valid(&committee),
                    "tampered cert must fail is_valid, not decode as valid"
                );
            }
            Err(_) => {
                // Decode-time failure is also acceptable — never a valid cert.
            }
        }
    }

    #[test]
    fn tampered_standalone_vote_fails_when_assembled() {
        let t = tx("alice", 0, "bob", 10);
        let (committee, mut auth) = setup(4);
        let good: Vec<Vote> = auth
            .iter_mut()
            .take(2)
            .map(|a| a.handle(&t).unwrap())
            .collect();
        let mut v2 = auth[2].handle(&t).unwrap();
        let mut wire = encode_vote(&v2);
        // Corrupt signature bytes at the end of the vote stream.
        let last = wire.len() - 1;
        wire[last] ^= 0x5a;
        match decode_vote(&wire) {
            Ok(bad) => {
                v2 = bad;
                let mut all = good;
                all.push(v2);
                assert!(
                    Certificate::assemble(t, all, &committee).is_none(),
                    "tampered vote must not form a valid certificate"
                );
            }
            Err(_) => {
                // Fine: codec refused the stream.
            }
        }
    }

    #[test]
    fn wire_layout_is_independent_of_signing_message_but_vote_still_verifies() {
        // Sanity: round-tripped vote still verifies under the same signing_message.
        let t = tx("alice", 0, "bob", 3);
        let sk = key(9);
        let v = Vote {
            authority: "a9".into(),
            transfer: t.clone(),
            signature: sk.sign(&signing_message(&t)),
        };
        let back = decode_vote(&encode_vote(&v)).unwrap();
        assert_eq!(back.signature, v.signature);
        sk.verifying_key()
            .verify_strict(&signing_message(&t), &back.signature)
            .unwrap();
    }
}
