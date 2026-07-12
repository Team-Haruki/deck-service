use std::ffi::{CStr, CString};
use std::os::raw::c_char;

pub type DeckRecommendHandle = *mut std::ffi::c_void;

unsafe extern "C" {
    pub fn deck_recommend_init_data_path(path: *const c_char) -> *const c_char;
    pub fn deck_recommend_create() -> DeckRecommendHandle;
    pub fn deck_recommend_destroy(handle: DeckRecommendHandle);
    pub fn deck_recommend_update_masterdata(
        handle: DeckRecommendHandle,
        base_dir: *const c_char,
        region: *const c_char,
    ) -> *const c_char;
    pub fn deck_recommend_update_masterdata_from_json(
        handle: DeckRecommendHandle,
        json_map: *const c_char,
        region: *const c_char,
    ) -> *const c_char;
    pub fn deck_recommend_update_masterdata_from_json_n(
        handle: DeckRecommendHandle,
        json_map: *const c_char,
        json_map_len: usize,
        region: *const c_char,
    ) -> *const c_char;
    pub fn deck_recommend_update_musicmetas(
        handle: DeckRecommendHandle,
        file_path: *const c_char,
        region: *const c_char,
    ) -> *const c_char;
    pub fn deck_recommend_update_musicmetas_from_string(
        handle: DeckRecommendHandle,
        json_str: *const c_char,
        region: *const c_char,
    ) -> *const c_char;
    pub fn deck_recommend_update_musicmetas_from_string_n(
        handle: DeckRecommendHandle,
        json_str: *const c_char,
        json_str_len: usize,
        region: *const c_char,
    ) -> *const c_char;
    pub fn deck_recommend_cache_userdata(
        handle: DeckRecommendHandle,
        userdata_json: *const c_char,
        hash_out: *mut *const c_char,
    ) -> *const c_char;
    pub fn deck_recommend_cache_userdata_n(
        handle: DeckRecommendHandle,
        userdata_json: *const c_char,
        userdata_json_len: usize,
        hash_out: *mut *const c_char,
        hash_len_out: *mut usize,
    ) -> *const c_char;
    pub fn deck_recommend_attach_cached_userdata(
        handle: DeckRecommendHandle,
        userdata_hash: *const c_char,
    ) -> *const c_char;
    pub fn deck_recommend_attach_cached_userdata_n(
        handle: DeckRecommendHandle,
        userdata_hash: *const c_char,
        userdata_hash_len: usize,
    ) -> *const c_char;
    pub fn deck_recommend_recommend(
        handle: DeckRecommendHandle,
        options_json: *const c_char,
        error_out: *mut *const c_char,
    ) -> *const c_char;
    pub fn deck_recommend_recommend_n(
        handle: DeckRecommendHandle,
        options_json: *const c_char,
        options_json_len: usize,
        error_out: *mut *const c_char,
        result_len_out: *mut usize,
    ) -> *const c_char;
    pub fn deck_recommend_recommend_with_default_timeout(
        handle: DeckRecommendHandle,
        options_json: *const c_char,
        default_timeout_ms: std::os::raw::c_int,
        error_out: *mut *const c_char,
    ) -> *const c_char;
    pub fn deck_recommend_recommend_with_default_timeout_n(
        handle: DeckRecommendHandle,
        options_json: *const c_char,
        options_json_len: usize,
        default_timeout_ms: std::os::raw::c_int,
        error_out: *mut *const c_char,
        result_len_out: *mut usize,
    ) -> *const c_char;
    pub fn deck_recommend_recommend_with_context(
        handle: DeckRecommendHandle,
        options_json: *const c_char,
        forced_region: *const c_char,
        forced_userdata_hash: *const c_char,
        default_timeout_ms: std::os::raw::c_int,
        error_out: *mut *const c_char,
    ) -> *const c_char;
    pub fn deck_recommend_recommend_with_context_n(
        handle: DeckRecommendHandle,
        options_json: *const c_char,
        options_json_len: usize,
        forced_region: *const c_char,
        forced_region_len: usize,
        forced_userdata_hash: *const c_char,
        forced_userdata_hash_len: usize,
        default_timeout_ms: std::os::raw::c_int,
        error_out: *mut *const c_char,
        result_len_out: *mut usize,
    ) -> *const c_char;
    pub fn deck_recommend_recommend_batch_with_context_n(
        handle: DeckRecommendHandle,
        options_json: *const c_char,
        options_json_len: usize,
        forced_region: *const c_char,
        forced_region_len: usize,
        forced_userdata_hash: *const c_char,
        forced_userdata_hash_len: usize,
        default_timeout_ms: std::os::raw::c_int,
        error_out: *mut *const c_char,
        result_len_out: *mut usize,
    ) -> *const c_char;
    pub fn deck_recommend_calculate(
        handle: DeckRecommendHandle,
        options_json: *const c_char,
        error_out: *mut *const c_char,
    ) -> *const c_char;
    pub fn deck_recommend_calculate_n(
        handle: DeckRecommendHandle,
        options_json: *const c_char,
        options_json_len: usize,
        error_out: *mut *const c_char,
        result_len_out: *mut usize,
    ) -> *const c_char;
    pub fn deck_recommend_get_world_bloom_support_cards(
        handle: DeckRecommendHandle,
        options_json: *const c_char,
        error_out: *mut *const c_char,
    ) -> *const c_char;
    pub fn deck_recommend_get_world_bloom_support_cards_n(
        handle: DeckRecommendHandle,
        options_json: *const c_char,
        options_json_len: usize,
        error_out: *mut *const c_char,
        result_len_out: *mut usize,
    ) -> *const c_char;
    pub fn deck_recommend_free_string(str_ptr: *const c_char);
}

/// Convert a C error string to a Rust Result, freeing the C string.
///
/// # Safety
/// `err` must be either null or a valid pointer returned from the C bridge.
pub unsafe fn check_error(err: *const c_char) -> Result<(), String> {
    if err.is_null() {
        Ok(())
    } else {
        let msg = unsafe { CStr::from_ptr(err) }
            .to_string_lossy()
            .into_owned();
        unsafe { deck_recommend_free_string(err) };
        Err(msg)
    }
}

/// Convert a Rust &str to a CString, panicking on embedded NULs.
pub fn to_cstring(s: &str) -> CString {
    CString::new(s).expect("string contains NUL byte")
}
