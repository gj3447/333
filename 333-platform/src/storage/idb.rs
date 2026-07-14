// KG: CONTRACT_333_IndexedDB — 브라우저 IndexedDB 바인딩
// KG: plan-333-longinus-full-coverage-2026-04-14 — src-storage-idb-WasmIdbStore
// JS 측에서 실제 IDB 조작, Rust는 wasm_bindgen extern으로 호출
// cfg(target_arch = "wasm32") — p2p/mod.rs에서 조건 컴파일

use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    /// JS 측 IDB helper (pkg/index.html에서 구현)
    #[wasm_bindgen(js_namespace = tripleThreeIdb)]
    async fn idb_put(key: &str, value: &[u8]) -> JsValue;

    #[wasm_bindgen(js_namespace = tripleThreeIdb)]
    async fn idb_get(key: &str) -> JsValue;

    #[wasm_bindgen(js_namespace = tripleThreeIdb)]
    async fn idb_delete(key: &str) -> JsValue;
}

/// WASM-side IDB store wrapper
#[wasm_bindgen]
pub struct WasmIdbStore;

#[wasm_bindgen]
impl WasmIdbStore {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self
    }

    pub async fn put(&self, key: &str, value: &[u8]) -> bool {
        let result = idb_put(key, value).await;
        !result.is_null()
    }

    pub async fn get(&self, key: &str) -> Option<Vec<u8>> {
        let result = idb_get(key).await;
        if result.is_null() || result.is_undefined() {
            None
        } else {
            let arr = js_sys::Uint8Array::new(&result);
            Some(arr.to_vec())
        }
    }

    pub async fn delete(&self, key: &str) -> bool {
        let result = idb_delete(key).await;
        !result.is_null()
    }
}
