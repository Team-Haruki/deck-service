use std::collections::HashMap;
use std::ffi::CStr;
use std::time::Instant;

use serde::Serialize;

use crate::ffi;

/// Safe wrapper around the C++ SekaiDeckRecommend instance.
/// This type is Send but not Sync — it must be protected by a Mutex for shared access.
pub struct DeckRecommend {
    handle: ffi::DeckRecommendHandle,
}

unsafe impl Send for DeckRecommend {}

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
        let result = ffi::check_error(err);
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
        let result = ffi::check_error(err);
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
        let c_json = ffi::to_cstring(&json_str);
        let c_region = ffi::to_cstring(region);
        let err = unsafe {
            ffi::deck_recommend_update_masterdata_from_json(
                self.handle,
                c_json.as_ptr(),
                c_region.as_ptr(),
            )
        };
        let result = ffi::check_error(err);
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
        let result = ffi::check_error(err);
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
        let c_data = ffi::to_cstring(data);
        let c_region = ffi::to_cstring(region);
        let err = unsafe {
            ffi::deck_recommend_update_musicmetas_from_string(
                self.handle,
                c_data.as_ptr(),
                c_region.as_ptr(),
            )
        };
        let result = ffi::check_error(err);
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
        let c_data = ffi::to_cstring(data);
        let mut hash_out: *const std::os::raw::c_char = std::ptr::null();
        let err = unsafe {
            ffi::deck_recommend_cache_userdata(self.handle, c_data.as_ptr(), &mut hash_out)
        };
        ffi::check_error(err)?;
        if hash_out.is_null() {
            return Err("deck_recommend_cache_userdata returned empty hash".into());
        }

        let hash = unsafe { CStr::from_ptr(hash_out) }
            .to_string_lossy()
            .into_owned();
        unsafe { ffi::deck_recommend_free_string(hash_out) };
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
        let started = Instant::now();
        tracing::debug!(options_bytes = options_json.len(), "ffi recommend start");
        let c_opts = ffi::to_cstring(options_json);
        let mut error_out: *const std::os::raw::c_char = std::ptr::null();

        let result_ptr =
            unsafe { ffi::deck_recommend_recommend(self.handle, c_opts.as_ptr(), &mut error_out) };

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

        let result = unsafe { CStr::from_ptr(result_ptr) }
            .to_string_lossy()
            .into_owned();
        unsafe { ffi::deck_recommend_free_string(result_ptr) };
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
}

impl Drop for DeckRecommend {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { ffi::deck_recommend_destroy(self.handle) };
        }
    }
}
