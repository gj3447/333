use ed25519_dalek::SigningKey;
use transfer333::{
    encode_certificate, Authority, Certificate, Committee, Ledger, NetworkId, OwnerRegistry,
    SignedTransfer, Transfer, TransferPolicy,
};
fn key(s: u8) -> SigningKey { SigningKey::from_bytes(&[s; 32]) }
fn hx(b: &[u8]) -> String { b.iter().map(|x| format!("{x:02x}")).collect() }
fn signer(id: &str) -> SigningKey { match id { "a0"=>key(1),"a1"=>key(2),"a2"=>key(3),_=>unreachable!() } }
fn cert_bytes(auths: &mut [Authority], c: &Committee, o: &SignedTransfer) -> Vec<u8> {
    let votes: Vec<_> = auths.iter_mut().filter_map(|a| a.handle(o).ok()).collect();
    encode_certificate(&Certificate::assemble(o.clone(), votes, c).expect("assemble"))
}
fn main() {
    let net = "rt2-e2e";
    let browser_pub = transfer333_wasm::browser_owner_pubkey_hex(&[7u8;32]);
    let bk = { let r:Vec<u8>=(0..browser_pub.len()).step_by(2).map(|i|u8::from_str_radix(&browser_pub[i..i+2],16).unwrap()).collect();
        ed25519_dalek::VerifyingKey::from_bytes(&r.try_into().unwrap()).unwrap() };
    let owners = vec![("treasury".to_string(), key(42).verifying_key()), ("browser-payee".to_string(), bk)];
    let policy = TransferPolicy::new(NetworkId::new(net).unwrap(), OwnerRegistry::new(owners.clone()).unwrap());
    let ids = ["a0","a1","a2"];
    let committee = Committee::new(ids.iter().map(|i|(i.to_string(),signer(i).verifying_key())), policy.clone()).unwrap();
    let genesis = || Ledger::genesis([("treasury".to_string(),333u128),("browser-payee".to_string(),0)]);
    let mut auths: Vec<Authority> = ids.iter().map(|i|Authority::new(i.to_string(),signer(i),policy.clone(),committee.id(),genesis())).collect();
    let order = SignedTransfer::sign(&policy, Transfer{from:"treasury".into(),from_seq:0,to:"browser-payee".into(),amount:1}, &key(42));
    let honest = cert_bytes(&mut auths, &committee, &order);
    let rsign = |id:&str| match id {"r0"=>key(90),"r1"=>key(91),"r2"=>key(92),_=>unreachable!()};
    let rids=["r0","r1","r2"];
    let rc = Committee::new(rids.iter().map(|i|(i.to_string(),rsign(i).verifying_key())), policy.clone()).unwrap();
    let mut ra:Vec<Authority>=rids.iter().map(|i|Authority::new(i.to_string(),rsign(i),policy.clone(),rc.id(),genesis())).collect();
    let rogue = cert_bytes(&mut ra, &rc, &order);
    let owners_json: Vec<String> = owners.iter().map(|(a,k)|format!("\"{a}\":\"{}\"",hx(k.as_bytes()))).collect();
    let auth_json: Vec<String> = ids.iter().map(|i|format!("\"{i}\":\"{}\"",hx(signer(i).verifying_key().as_bytes()))).collect();
    let config = format!("{{\"network_id\":\"{net}\",\"owners\":{{{}}},\"authorities\":{{{}}}}}", owners_json.join(","), auth_json.join(","));
    println!("{{\"config\":{config},\"honest_cert_hex\":\"{}\",\"rogue_cert_hex\":\"{}\"}}", hx(&honest), hx(&rogue));
}
