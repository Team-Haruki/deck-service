#ifndef DECK_RECOMMEND_C_H
#define DECK_RECOMMEND_C_H

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

// Opaque handle to SekaiDeckRecommend instance
typedef void* DeckRecommendHandle;

// Initialize the static data directory (must be called before any other function)
// Returns NULL on success, or an error message string (caller must free with deck_recommend_free_string)
const char* deck_recommend_init_data_path(const char* path);

// Create a new SekaiDeckRecommend instance
// Returns handle, or NULL on failure
DeckRecommendHandle deck_recommend_create(void);

// Destroy a SekaiDeckRecommend instance
void deck_recommend_destroy(DeckRecommendHandle handle);

// Update master data from a local directory
// Returns NULL on success, or an error message (caller must free)
const char* deck_recommend_update_masterdata(DeckRecommendHandle handle, const char* base_dir, const char* region);

// Update master data from a JSON string: {"key": "json_content", ...}
// Returns NULL on success, or an error message (caller must free)
const char* deck_recommend_update_masterdata_from_json(DeckRecommendHandle handle, const char* json_map, const char* region);
const char* deck_recommend_update_masterdata_from_json_n(
    DeckRecommendHandle handle,
    const char* json_map,
    size_t json_map_len,
    const char* region
);

// Update music metas from a local file
// Returns NULL on success, or an error message (caller must free)
const char* deck_recommend_update_musicmetas(DeckRecommendHandle handle, const char* file_path, const char* region);

// Update music metas from a JSON string
// Returns NULL on success, or an error message (caller must free)
const char* deck_recommend_update_musicmetas_from_string(DeckRecommendHandle handle, const char* json_str, const char* region);
const char* deck_recommend_update_musicmetas_from_string_n(
    DeckRecommendHandle handle,
    const char* json_str,
    size_t json_str_len,
    const char* region
);

// Parse and cache user data by hash for later recommend calls.
// On success, *hash_out is set to the returned userdata hash (caller must free).
const char* deck_recommend_cache_userdata(DeckRecommendHandle handle, const char* userdata_json, const char** hash_out);
const char* deck_recommend_cache_userdata_n(
    DeckRecommendHandle handle,
    const char* userdata_json,
    size_t userdata_json_len,
    const char** hash_out,
    size_t* hash_len_out
);

// Run deck recommendation. options_json is the full options as a JSON string.
// Returns a JSON string with the result (caller must free), or NULL on failure.
// If error occurs, *error_out is set to an error message (caller must free).
const char* deck_recommend_recommend(DeckRecommendHandle handle, const char* options_json, const char** error_out);
const char* deck_recommend_recommend_n(
    DeckRecommendHandle handle,
    const char* options_json,
    size_t options_json_len,
    const char** error_out,
    size_t* result_len_out
);

// Run deck recommendation with an optional default timeout. If default_timeout_ms <= 0,
// no default timeout is applied. Returns a JSON result string (caller must free),
// or NULL on failure. If error occurs, *error_out is set to an error message.
const char* deck_recommend_recommend_with_default_timeout(
    DeckRecommendHandle handle,
    const char* options_json,
    int default_timeout_ms,
    const char** error_out
);
const char* deck_recommend_recommend_with_default_timeout_n(
    DeckRecommendHandle handle,
    const char* options_json,
    size_t options_json_len,
    int default_timeout_ms,
    const char** error_out,
    size_t* result_len_out
);

// Run deck recommendation with request context supplied outside options_json.
// forced_region and forced_userdata_hash override same-named fields in options_json
// when non-empty. default_timeout_ms is only used when options_json omits timeout_ms.
const char* deck_recommend_recommend_with_context(
    DeckRecommendHandle handle,
    const char* options_json,
    const char* forced_region,
    const char* forced_userdata_hash,
    int default_timeout_ms,
    const char** error_out
);
const char* deck_recommend_recommend_with_context_n(
    DeckRecommendHandle handle,
    const char* options_json,
    size_t options_json_len,
    const char* forced_region,
    size_t forced_region_len,
    const char* forced_userdata_hash,
    size_t forced_userdata_hash_len,
    int default_timeout_ms,
    const char** error_out,
    size_t* result_len_out
);

// Run fixed-deck calculations. options_json is the full options as a JSON string.
// Supported modes: "deck", "challenge", "live_full".
// Returns a JSON string with the result (caller must free), or NULL on failure.
// If error occurs, *error_out is set to an error message (caller must free).
const char* deck_recommend_calculate(DeckRecommendHandle handle, const char* options_json, const char** error_out);
const char* deck_recommend_calculate_n(
    DeckRecommendHandle handle,
    const char* options_json,
    size_t options_json_len,
    const char** error_out,
    size_t* result_len_out
);

// Get world bloom support deck cards. options_json is the full options as a JSON string.
// Returns a JSON array with the result (caller must free), or NULL on failure.
// If error occurs, *error_out is set to an error message (caller must free).
const char* deck_recommend_get_world_bloom_support_cards(DeckRecommendHandle handle, const char* options_json, const char** error_out);
const char* deck_recommend_get_world_bloom_support_cards_n(
    DeckRecommendHandle handle,
    const char* options_json,
    size_t options_json_len,
    const char** error_out,
    size_t* result_len_out
);

// Free a string returned by any of the above functions
void deck_recommend_free_string(const char* str);

#ifdef __cplusplus
}
#endif

#endif // DECK_RECOMMEND_C_H
