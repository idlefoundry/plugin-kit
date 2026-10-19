// The AU adapter must preserve channel pitch bend when the CLAP port accepts MIDI.
// Exercise the real adapter/event list, without a synth, editor or audio device.
#include "detail/auv2/process.h"

#include <cmath>
#include <cstring>
#include <iostream>
#include <stdexcept>
#include <string>
#include <vector>

using Clap::AUv2::ProcessAdapter;
using Clap::AUv2::clap_multi_event_t;

namespace
{
void require(bool condition, const char *message)
{
  if (!condition) throw std::runtime_error(message);
}

clap_process_status capture(const clap_plugin_t *plugin, const clap_process_t *data)
{
  auto &events = *static_cast<std::vector<clap_multi_event_t> *>(plugin->plugin_data);
  for (uint32_t i = 0; i < data->in_events->size(data->in_events); ++i)
  {
    const auto *event = data->in_events->get(data->in_events, i);
    require(event && event->size <= sizeof(clap_multi_event_t), "invalid input event");
    clap_multi_event_t copy{};
    std::memcpy(&copy, event, event->size);
    events.push_back(copy);
  }
  return CLAP_PROCESS_CONTINUE;
}

struct Host
{
  std::vector<clap_multi_event_t> events;
  clap_plugin_t plugin{};
  ausdk::AUScope inputs, outputs;
  Clap::AUv2::ParameterTree parameters;
  ProcessAdapter adapter;

  Host(uint32_t preferred, uint32_t supported)
  {
    plugin.plugin_data = &events;
    plugin.process = capture;
    adapter.setupProcessing(inputs, outputs, &plugin, nullptr, nullptr, &parameters, nullptr,
                            256, preferred, supported, 0, 0);
  }

  void render()
  {
    events.clear();
    AudioUnitRenderActionFlags flags = 0;
    AudioTimeStamp timestamp{};
    Clap::AUv2::ProcessData data{flags, timestamp};
    data.numSamples = 256;
    adapter.process(data);
  }
};

void rawBends(uint32_t preferred, uint32_t supported)
{
  Host host(preferred, supported);
  for (uint8_t channel : {0, 3, 15})
  {
    for (uint16_t bend : {0, 8192, 16383, 4660})
    {
      host.adapter.addMIDIEvent(0xE0 | channel, bend & 0x7F, bend >> 7, 17);
      host.render();
      require(host.events.size() == 1, "one bend must produce exactly one event");
      const auto &e = host.events.front();
      require(e.header.type == CLAP_EVENT_MIDI,
              "channel pitch bend became note-expression tuning instead of raw MIDI");
      require(e.header.size == sizeof(clap_event_midi_t), "wrong MIDI event size");
      require(e.header.space_id == CLAP_CORE_EVENT_SPACE_ID && e.header.time == 17,
              "bend namespace/sample offset changed");
      require(e.midi.port_index == 0 && e.midi.data[0] == (0xE0 | channel) &&
                  e.midi.data[1] == (bend & 0x7F) && e.midi.data[2] == (bend >> 7),
              "bend channel or 14-bit value changed");
    }
  }
}

void clapOnlyFallback()
{
  Host host(CLAP_NOTE_DIALECT_CLAP, CLAP_NOTE_DIALECT_CLAP);
  for (uint16_t bend : {0, 8192, 16383})
  {
    host.adapter.addMIDIEvent(0xE7, bend & 0x7F, bend >> 7, 9);
    host.render();
    require(host.events.size() == 1, "one fallback bend must produce one event");
    const auto &e = host.events.front();
    require(e.header.type == CLAP_EVENT_NOTE_EXPRESSION &&
                e.noteexpression.expression_id == CLAP_NOTE_EXPRESSION_TUNING,
            "CLAP-only ports still need their tuning fallback");
    require(e.noteexpression.channel == 7 && e.noteexpression.key == -1 &&
                e.noteexpression.note_id == -1 && e.header.time == 9,
            "fallback addressing/sample offset changed");
    require(std::abs(e.noteexpression.value - (bend - 8192) / 8192.0 * 2.0) < 1e-12,
            "CLAP-only fallback tuning changed");
  }
}

void otherEvents()
{
  Host host(CLAP_NOTE_DIALECT_CLAP, CLAP_NOTE_DIALECT_CLAP | CLAP_NOTE_DIALECT_MIDI);
  host.adapter.addMIDIEvent(0x93, 69, 72, 5);
  host.adapter.addMIDIEvent(0x93, 69, 0, 200);
  for (uint8_t cc : {1, 64, 120, 121, 123}) host.adapter.addMIDIEvent(0xB3, cc, 127, 10);
  host.adapter.addMIDIEvent(0xA3, 69, 80, 20);
  host.adapter.addMIDIEvent(0xD3, 90, 0, 21);
  host.render();
  require(host.events.size() == 9, "notes/controllers/pressure were lost or duplicated");
  require(host.events.front().header.type == CLAP_EVENT_NOTE_ON &&
              host.events.front().note.channel == 3 && host.events.front().note.key == 69,
          "typed note delivery changed");
  require(host.events.back().header.type == CLAP_EVENT_NOTE_OFF &&
              host.events.back().header.time == 200,
          "velocity-zero note-off or event sorting changed");
  size_t i = 1;
  for (uint8_t cc : {1, 64, 120, 121, 123})
  {
    const auto &e = host.events[i++];
    require(e.header.type == CLAP_EVENT_MIDI && e.midi.data[0] == 0xB3 &&
                e.midi.data[1] == cc && e.midi.data[2] == 127 && e.header.time == 10,
            "modulation/sustain/reset/panic CC delivery changed");
  }
  require(host.events[6].noteexpression.expression_id == CLAP_NOTE_EXPRESSION_PRESSURE &&
              host.events[6].noteexpression.key == 69 &&
              host.events[7].noteexpression.expression_id == CLAP_NOTE_EXPRESSION_PRESSURE &&
              host.events[7].noteexpression.key == -1,
          "pressure translation changed");
}
} // namespace

int main()
{
  int failures = 0;
  const auto test = [&](const char *name, auto body) {
    try { body(); std::cout << "PASS " << name << '\n'; }
    catch (const std::exception &e) { ++failures; std::cerr << "FAIL " << name << ": " << e.what() << '\n'; }
  };
  test("CLAP-preferred plus MIDI (nih-plug instruments)", [] { rawBends(CLAP_NOTE_DIALECT_CLAP, CLAP_NOTE_DIALECT_CLAP | CLAP_NOTE_DIALECT_MIDI); });
  test("MIDI-preferred", [] { rawBends(CLAP_NOTE_DIALECT_MIDI, CLAP_NOTE_DIALECT_MIDI); });
  test("CLAP-preferred plus MPE", [] { rawBends(CLAP_NOTE_DIALECT_CLAP, CLAP_NOTE_DIALECT_CLAP | CLAP_NOTE_DIALECT_MIDI_MPE); });
  test("CLAP-only fallback", clapOnlyFallback);
  test("other notes/controllers/pressure", otherEvents);
  return failures ? 1 : 0;
}
