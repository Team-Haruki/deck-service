use std::collections::HashMap;
use std::ffi::CStr;
use std::os::raw::c_char;
use std::time::Instant;

use serde::Serialize;

use crate::ffi;

/// Safe wrapper around the C++ SekaiDeckRecommend instance.
/// This type is Send but not Sync — it must be protected by a Mutex for shared access.
pub struct DeckRecommend {
    handle: ffi::DeckRecommendHandle,
}

unsafe impl Send for DeckRecommend {}

struct FfiString {
    ptr: *const c_char,
    len: Option<usize>,
}

impl FfiString {
    fn from_ptr_len(ptr: *const c_char, len: usize) -> Self {
        Self {
            ptr,
            len: Some(len),
        }
    }

    fn into_string(mut self) -> Result<String, String> {
        if self.ptr.is_null() {
            return Err("C bridge returned a null string".into());
        }

        let result = match self.len {
            Some(len) => {
                let bytes =
                    unsafe { std::slice::from_raw_parts(self.ptr.cast::<u8>(), len) }.to_vec();
                String::from_utf8(bytes).map_err(|err| err.to_string())?
            }
            None => unsafe { CStr::from_ptr(self.ptr) }
                .to_string_lossy()
                .into_owned(),
        };
        unsafe { ffi::deck_recommend_free_string(self.ptr) };
        self.ptr = std::ptr::null();
        Ok(result)
    }
}

impl Drop for FfiString {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe { ffi::deck_recommend_free_string(self.ptr) };
            self.ptr = std::ptr::null();
        }
    }
}

impl DeckRecommend {
    pub fn new() -> Result<Self, String> {
        let handle = unsafe { ffi::deck_recommend_create() };
        if handle.is_null() {
            return Err("Failed to create DeckRecommend instance".into());
        }
        Ok(Self { handle })
    }

    pub fn init_data_path(path: &str) -> Result<(), String> {
        let started = Instant::now();
        tracing::debug!(data_path = %path, "ffi init_data_path start");
        let c_path = ffi::to_cstring(path);
        let err = unsafe { ffi::deck_recommend_init_data_path(c_path.as_ptr()) };
        let result = unsafe { ffi::check_error(err) };
        if result.is_ok() {
            tracing::debug!(
                elapsed_ms = started.elapsed().as_secs_f64() * 1000.0,
                "ffi init_data_path completed"
            );
        }
        result
    }

    pub fn update_masterdata(&self, base_dir: &str, region: &str) -> Result<(), String> {
        let started = Instant::now();
        tracing::debug!(region = %region, base_dir = %base_dir, "ffi update_masterdata start");
        let c_dir = ffi::to_cstring(base_dir);
        let c_region = ffi::to_cstring(region);
        let err = unsafe {
            ffi::deck_recommend_update_masterdata(self.handle, c_dir.as_ptr(), c_region.as_ptr())
        };
        let result = unsafe { ffi::check_error(err) };
        if result.is_ok() {
            tracing::debug!(
                region = %region,
                elapsed_ms = started.elapsed().as_secs_f64() * 1000.0,
                "ffi update_masterdata completed"
            );
        }
        result
    }

    pub fn update_masterdata_from_json(
        &self,
        data: &HashMap<String, String>,
        region: &str,
    ) -> Result<(), String> {
        let started = Instant::now();
        tracing::debug!(
            region = %region,
            file_count = data.len(),
            "ffi update_masterdata_from_json start"
        );
        let json_str = sonic_rs::to_string(data).map_err(|e| e.to_string())?;
        let c_region = ffi::to_cstring(region);
        let err = unsafe {
            ffi::deck_recommend_update_masterdata_from_json_n(
                self.handle,
                json_str.as_ptr().cast(),
                json_str.len(),
                c_region.as_ptr(),
            )
        };
        let result = unsafe { ffi::check_error(err) };
        if result.is_ok() {
            tracing::debug!(
                region = %region,
                json_bytes = json_str.len(),
                elapsed_ms = started.elapsed().as_secs_f64() * 1000.0,
                "ffi update_masterdata_from_json completed"
            );
        }
        result
    }

    pub fn update_musicmetas(&self, file_path: &str, region: &str) -> Result<(), String> {
        let started = Instant::now();
        tracing::debug!(region = %region, file_path = %file_path, "ffi update_musicmetas start");
        let c_path = ffi::to_cstring(file_path);
        let c_region = ffi::to_cstring(region);
        let err = unsafe {
            ffi::deck_recommend_update_musicmetas(self.handle, c_path.as_ptr(), c_region.as_ptr())
        };
        let result = unsafe { ffi::check_error(err) };
        if result.is_ok() {
            tracing::debug!(
                region = %region,
                elapsed_ms = started.elapsed().as_secs_f64() * 1000.0,
                "ffi update_musicmetas completed"
            );
        }
        result
    }

    pub fn update_musicmetas_from_string(&self, data: &str, region: &str) -> Result<(), String> {
        let started = Instant::now();
        tracing::debug!(
            region = %region,
            data_bytes = data.len(),
            "ffi update_musicmetas_from_string start"
        );
        let c_region = ffi::to_cstring(region);
        let err = unsafe {
            ffi::deck_recommend_update_musicmetas_from_string_n(
                self.handle,
                data.as_ptr().cast(),
                data.len(),
                c_region.as_ptr(),
            )
        };
        let result = unsafe { ffi::check_error(err) };
        if result.is_ok() {
            tracing::debug!(
                region = %region,
                elapsed_ms = started.elapsed().as_secs_f64() * 1000.0,
                "ffi update_musicmetas_from_string completed"
            );
        }
        result
    }

    pub fn cache_userdata(&self, data: &str) -> Result<String, String> {
        let started = Instant::now();
        tracing::debug!(data_bytes = data.len(), "ffi cache_userdata start");
        let mut hash_out: *const std::os::raw::c_char = std::ptr::null();
        let mut hash_len = 0usize;
        let err = unsafe {
            ffi::deck_recommend_cache_userdata_n(
                self.handle,
                data.as_ptr().cast(),
                data.len(),
                &mut hash_out,
                &mut hash_len,
            )
        };
        unsafe { ffi::check_error(err) }?;
        if hash_out.is_null() {
            return Err("deck_recommend_cache_userdata returned empty hash".into());
        }

        let hash = FfiString::from_ptr_len(hash_out, hash_len).into_string()?;
        tracing::debug!(
            hash_prefix = %hash.chars().take(8).collect::<String>(),
            elapsed_ms = started.elapsed().as_secs_f64() * 1000.0,
            "ffi cache_userdata completed"
        );
        Ok(hash)
    }

    /// Run deck recommendation with a JSON options object.
    /// Returns the raw JSON result string.
    pub fn recommend_raw(&self, options_json: &str) -> Result<String, String> {
        self.recommend_raw_with_default_timeout(options_json, None)
    }

    /// Run deck recommendation with a JSON options object and an optional
    /// default timeout applied by the C++ bridge when the payload omits it.
    pub fn recommend_raw_with_default_timeout(
        &self,
        options_json: &str,
        default_timeout_ms: Option<i32>,
    ) -> Result<String, String> {
        self.recommend_raw_with_context(options_json, None, None, default_timeout_ms)
    }

    /// Run deck recommendation with request context supplied outside the JSON
    /// payload. Non-empty region/userdata_hash override same-named JSON fields.
    pub fn recommend_raw_with_context(
        &self,
        options_json: &str,
        forced_region: Option<&str>,
        forced_userdata_hash: Option<&str>,
        default_timeout_ms: Option<i32>,
    ) -> Result<String, String> {
        let started = Instant::now();
        tracing::debug!(
            options_bytes = options_json.len(),
            forced_region = forced_region.unwrap_or(""),
            hash_prefix = %forced_userdata_hash
                .map(|hash| hash.chars().take(8).collect::<String>())
                .unwrap_or_default(),
            default_timeout_ms = default_timeout_ms.unwrap_or_default(),
            "ffi recommend start"
        );
        let mut error_out: *const std::os::raw::c_char = std::ptr::null();
        let mut result_len = 0usize;
        let default_timeout_ms = default_timeout_ms.unwrap_or_default();

        let result_ptr = if forced_region.is_some() || forced_userdata_hash.is_some() {
            unsafe {
                ffi::deck_recommend_recommend_with_context_n(
                    self.handle,
                    options_json.as_ptr().cast(),
                    options_json.len(),
                    forced_region
                        .map(|value| value.as_ptr().cast())
                        .unwrap_or(std::ptr::null()),
                    forced_region.map(str::len).unwrap_or_default(),
                    forced_userdata_hash
                        .map(|value| value.as_ptr().cast())
                        .unwrap_or(std::ptr::null()),
                    forced_userdata_hash.map(str::len).unwrap_or_default(),
                    default_timeout_ms,
                    &mut error_out,
                    &mut result_len,
                )
            }
        } else if default_timeout_ms > 0 {
            unsafe {
                ffi::deck_recommend_recommend_with_default_timeout_n(
                    self.handle,
                    options_json.as_ptr().cast(),
                    options_json.len(),
                    default_timeout_ms,
                    &mut error_out,
                    &mut result_len,
                )
            }
        } else {
            unsafe {
                ffi::deck_recommend_recommend_n(
                    self.handle,
                    options_json.as_ptr().cast(),
                    options_json.len(),
                    &mut error_out,
                    &mut result_len,
                )
            }
        };

        if result_ptr.is_null() {
            if !error_out.is_null() {
                let msg = unsafe { CStr::from_ptr(error_out) }
                    .to_string_lossy()
                    .into_owned();
                unsafe { ffi::deck_recommend_free_string(error_out) };
                tracing::debug!(
                    elapsed_ms = started.elapsed().as_secs_f64() * 1000.0,
                    error = %msg,
                    "ffi recommend returned error"
                );
                return Err(msg);
            }
            return Err("Unknown error during recommendation".into());
        }

        let result = FfiString::from_ptr_len(result_ptr, result_len).into_string()?;
        tracing::debug!(
            result_bytes = result.len(),
            elapsed_ms = started.elapsed().as_secs_f64() * 1000.0,
            "ffi recommend completed"
        );
        Ok(result)
    }

    /// Run deck recommendation with any serializable payload.
    pub fn recommend_value<T: Serialize>(
        &self,
        options: &T,
    ) -> Result<crate::models::DeckRecommendResult, String> {
        let json_str = sonic_rs::to_string(options).map_err(|e| e.to_string())?;
        let result_str = self.recommend_raw(&json_str)?;
        sonic_rs::from_str(&result_str).map_err(|e| e.to_string())
    }

    /// Run deck recommendation with typed options.
    pub fn recommend(
        &self,
        options: &crate::models::DeckRecommendOptions,
    ) -> Result<crate::models::DeckRecommendResult, String> {
        self.recommend_value(options)
    }

    pub fn calculate_raw(&self, options_json: &str) -> Result<String, String> {
        let started = Instant::now();
        tracing::debug!(options_bytes = options_json.len(), "ffi calculate start");
        let mut error_out: *const std::os::raw::c_char = std::ptr::null();
        let mut result_len = 0usize;

        let result_ptr = unsafe {
            ffi::deck_recommend_calculate_n(
                self.handle,
                options_json.as_ptr().cast(),
                options_json.len(),
                &mut error_out,
                &mut result_len,
            )
        };

        if result_ptr.is_null() {
            if !error_out.is_null() {
                let msg = unsafe { CStr::from_ptr(error_out) }
                    .to_string_lossy()
                    .into_owned();
                unsafe { ffi::deck_recommend_free_string(error_out) };
                tracing::debug!(
                    elapsed_ms = started.elapsed().as_secs_f64() * 1000.0,
                    error = %msg,
                    "ffi calculate returned error"
                );
                return Err(msg);
            }
            return Err("Unknown error during calculation".into());
        }

        let result = FfiString::from_ptr_len(result_ptr, result_len).into_string()?;
        tracing::debug!(
            result_bytes = result.len(),
            elapsed_ms = started.elapsed().as_secs_f64() * 1000.0,
            "ffi calculate completed"
        );
        Ok(result)
    }

    pub fn calculate_value<T: Serialize>(&self, options: &T) -> Result<sonic_rs::Value, String> {
        let json_str = sonic_rs::to_string(options).map_err(|e| e.to_string())?;
        let result_str = self.calculate_raw(&json_str)?;
        sonic_rs::from_str(&result_str).map_err(|e| e.to_string())
    }

    pub fn get_world_bloom_support_cards_raw(&self, options_json: &str) -> Result<String, String> {
        let started = Instant::now();
        tracing::debug!(
            options_bytes = options_json.len(),
            "ffi get_world_bloom_support_cards start"
        );
        let mut error_out: *const std::os::raw::c_char = std::ptr::null();
        let mut result_len = 0usize;

        let result_ptr = unsafe {
            ffi::deck_recommend_get_world_bloom_support_cards_n(
                self.handle,
                options_json.as_ptr().cast(),
                options_json.len(),
                &mut error_out,
                &mut result_len,
            )
        };

        if result_ptr.is_null() {
            if !error_out.is_null() {
                let msg = unsafe { CStr::from_ptr(error_out) }
                    .to_string_lossy()
                    .into_owned();
                unsafe { ffi::deck_recommend_free_string(error_out) };
                tracing::debug!(
                    elapsed_ms = started.elapsed().as_secs_f64() * 1000.0,
                    error = %msg,
                    "ffi get_world_bloom_support_cards returned error"
                );
                return Err(msg);
            }
            return Err("Unknown error during world bloom support cards calculation".into());
        }

        let result = FfiString::from_ptr_len(result_ptr, result_len).into_string()?;
        tracing::debug!(
            result_bytes = result.len(),
            elapsed_ms = started.elapsed().as_secs_f64() * 1000.0,
            "ffi get_world_bloom_support_cards completed"
        );
        Ok(result)
    }

    pub fn get_world_bloom_support_cards_value<T: Serialize>(
        &self,
        options: &T,
    ) -> Result<sonic_rs::Value, String> {
        let json_str = sonic_rs::to_string(options).map_err(|e| e.to_string())?;
        let result_str = self.get_world_bloom_support_cards_raw(&json_str)?;
        sonic_rs::from_str(&result_str).map_err(|e| e.to_string())
    }
}

impl Drop for DeckRecommend {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { ffi::deck_recommend_destroy(self.handle) };
        }
    }
}
