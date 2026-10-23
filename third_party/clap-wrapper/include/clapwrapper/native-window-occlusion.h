#pragma once

#include <stdint.h>

// Private host capability, version 1. Presence means the embedded editor may be
// presented remotely, so its native backing window's occlusion is not authoritative.
// The pointer is static for the host's lifetime. Clients may test presence alone;
// they need not read the structure or call into the host from their frame timer.
static const char CLAP_HOST_NATIVE_WINDOW_OCCLUSION_UNRELIABLE[] =
    "com.idlefoundry.native-window-occlusion-unreliable/1";

typedef struct clap_host_native_window_occlusion_unreliable
{
  uint32_t version;
} clap_host_native_window_occlusion_unreliable_t;
