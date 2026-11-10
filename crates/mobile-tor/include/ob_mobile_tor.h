// C ABI of crates/mobile-tor (src/ffi.rs). Strings are NUL-terminated UTF-8.
// Free every returned string with ob_tor_string_free and every ObTorBytes
// with ob_tor_bytes_free. ob_tor_exchange blocks: call it off the main thread.

#ifndef OB_MOBILE_TOR_H
#define OB_MOBILE_TOR_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct ObTorBytes {
  uint8_t *data;
  size_t len;
  char *error;
} ObTorBytes;

char *ob_tor_start(const char *data_dir);
char *ob_tor_status(void);
char *ob_tor_insert_discovery_key(const char *onion, const char *secret_base32);
ObTorBytes ob_tor_exchange(const char *onion, uint16_t port, const uint8_t *request, size_t request_len, size_t max_bytes, uint64_t timeout_ms);
// Lines about bootstrapping and dialing, without secrets. Copy the line: it is freed after `log` returns.
void ob_tor_set_log(void (*log)(const char *line));
void ob_tor_string_free(char *value);
void ob_tor_bytes_free(ObTorBytes value);

#ifdef __cplusplus
}
#endif

#endif
