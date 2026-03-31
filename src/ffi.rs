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
    pub fn deck_recommend_recommend(
        handle: DeckRecommendHandle,
        options_json: *const c_char,
        error_out: *mut *const c_char,
    ) -> *const c_char;
    pub fn deck_recommend_free_string(str_ptr: *const c_char);
}

/// Convert a C error string to a Rust Result, freeing the C string.
pub fn check_error(err: *const c_char) -> Result<(), String> {
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
