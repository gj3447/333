fn main() {
    let fx: String = std::fs::read_to_string("/tmp/rt2_fixture.json").unwrap();
    // config 필드만 추출 (조잡하게)
    let i = fx.find("\"config\":").unwrap() + 9;
    let rest = &fx[i..];
    // config는 {...}, 매칭 괄호까지
    let mut depth=0; let mut end=0;
    for (j,c) in rest.char_indices() { if c=='{'{depth+=1} if c=='}'{depth-=1; if depth==0 {end=j+1;break}} }
    let config = &rest[..end];
    println!("config = {config}");
    match transfer333_wasm::config_fingerprint_core(config) {
        Ok(fp) => println!("fp = {fp}"),
        Err(e) => println!("ERR = {e}"),
    }
}
