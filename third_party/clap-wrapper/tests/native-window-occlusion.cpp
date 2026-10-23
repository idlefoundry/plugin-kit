// The private host capability of change 7 (PATCHES.md): the AUv2 wrapper says that its backing
// window's occlusion is unreliable, and no other wrapper does. The CA-72's test
// (scripts/auv2/tests/occlusion.cpp there), on the real CLAP proxy dispatcher, without an editor,
// an audio device or a host. It needs only clap_proxy.cpp and the CLAP headers, on any system:
//
//   c++ -std=c++17 -DCLAP_WRAPPER_VERSION='"0.16.0"' -Isrc -Iinclude -I../clap/include \
//       tests/native-window-occlusion.cpp src/clap_proxy.cpp src/detail/clap/fsutil.cpp \
//       -o occlusion-test (and -ldl on Linux)
//
// as the kit's CI builds and runs it, or with a plug-in's component (its scripts/auv2).
#include "clap_proxy.h"
#include <clapwrapper/native-window-occlusion.h>
#include <iostream>
#include <stdexcept>

namespace {
void require(bool value, const char *message) {
  if (!value) throw std::runtime_error(message);
}
class LocalHost : public Clap::IHost {
 public:
  void mark_dirty() override {}
  void restartPlugin() override {}
  void request_callback() override {}
  void setupWrapperSpecifics(const clap_plugin_t *) override {}
  void setupAudioBusses(const clap_plugin_t *, const clap_plugin_audio_ports_t *) override {}
  void setupMIDIBusses(const clap_plugin_t *, const clap_plugin_note_ports_t *) override {}
  void setupParameters(const clap_plugin_t *, const clap_plugin_params_t *) override {}
  void param_rescan(clap_param_rescan_flags) override {}
  void param_clear(clap_id, clap_param_clear_flags) override {}
  void param_request_flush() override {}
  void latency_changed() override {}
  void tail_changed() override {}
  bool gui_can_resize() override { return false; }
  bool gui_request_resize(uint32_t, uint32_t) override { return false; }
  bool gui_request_show() override { return false; }
  bool gui_request_hide() override { return false; }
  bool register_timer(uint32_t, clap_id *) override { return false; }
  bool unregister_timer(clap_id) override { return false; }
  bool track_info_get(clap_track_info_t *) override { return false; }
  const char *host_get_name() override { return "unknown host"; }
  bool supportsContextMenu() const override { return false; }
  bool context_menu_populate(const clap_context_menu_target_t *, const clap_context_menu_builder_t *) override { return false; }
  bool context_menu_perform(const clap_context_menu_target_t *, clap_id) override { return false; }
  bool context_menu_can_popup() override { return false; }
  bool context_menu_popup(const clap_context_menu_target_t *, int32_t, int32_t, int32_t) override { return false; }
};
class RemoteHost : public LocalHost {
 public:
  bool native_window_occlusion_reliable() const override { return false; }
};
class Proxy : public Clap::Plugin {
 public:
  explicit Proxy(Clap::IHost *host) : Plugin(host) {}
  const clap_host_t *host() { return getClapHostInterface(); }
};
const void *query(Proxy &proxy, const char *id) {
  auto *host = proxy.host();
  return host->get_extension(host, id);
}
}

int main() {
  try {
    LocalHost local;
    RemoteHost remote;
    Proxy localProxy(&local), remoteProxy(&remote);
    require(local.native_window_occlusion_reliable(), "local wrapper lost its default policy");
    require(!query(localProxy, CLAP_HOST_NATIVE_WINDOW_OCCLUSION_UNRELIABLE),
            "a local wrapper advertised remote visibility");
    auto *marker = static_cast<const clap_host_native_window_occlusion_unreliable_t *>(
        query(remoteProxy, CLAP_HOST_NATIVE_WINDOW_OCCLUSION_UNRELIABLE));
    require(marker && marker->version == 1, "remote wrapper capability was not dispatched");
    require(!query(localProxy, CLAP_HOST_NATIVE_WINDOW_OCCLUSION_UNRELIABLE),
            "another instance's capability leaked into the local wrapper");
    require(!query(remoteProxy, "com.idlefoundry.native-window-occlusion-unreliable/2"),
            "an unknown capability version was advertised");
    require(query(localProxy, CLAP_EXT_GUI) && query(remoteProxy, CLAP_EXT_GUI),
            "the existing GUI host extension was lost");
    std::cout << "native occlusion capability: local, remote, version and instance isolation ... ok\n";
    return 0;
  } catch (const std::exception &error) {
    std::cerr << error.what() << '\n';
    return 1;
  }
}
