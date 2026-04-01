#ifndef DECK_RECOMMEND_C_H
#define DECK_RECOMMEND_C_H

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

// Update music metas from a local file
// Returns NULL on success, or an error message (caller must free)
const char* deck_recommend_update_musicmetas(DeckRecommendHandle handle, const char* file_path, const char* region);

// Update music metas from a JSON string
// Returns NULL on success, or an error message (caller must free)
const char* deck_recommend_update_musicmetas_from_string(DeckRecommendHandle handle, const char* json_str, const char* region);

// Parse and cache user data by hash for later recommend calls.
// On success, *hash_out is set to the returned userdata hash (caller must free).
const char* deck_recommend_cache_userdata(DeckRecommendHandle handle, const char* userdata_json, const char** hash_out);

// Run deck recommendation. options_json is the full options as a JSON string.
// Returns a JSON string with the result (caller must free), or NULL on failure.
// If error occurs, *error_out is set to an error message (caller must free).
const char* deck_recommend_recommend(DeckRecommendHandle handle, const char* options_json, const char** error_out);

// Free a string returned by any of the above functions
void deck_recommend_free_string(const char* str);

#ifdef __cplusplus
}
#endif

#endif // DECK_RECOMMEND_C_H
