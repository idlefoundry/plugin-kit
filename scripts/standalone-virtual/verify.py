#!/usr/bin/env python3
"""Opt-in Linux proof using only a private PipeWire graph and ALSA config.

No session manager, udev/device factory, real ALSA PCM or sequencer is loaded.
The graph deliberately links known constant playback into a virtual capture.
All subprocesses use a new runtime/config/state directory under /tmp.
"""
import json
import os
from pathlib import Path
import selectors
import shutil
import subprocess
import sys
import tempfile
import time


def main():
    if sys.platform != "linux":
        raise SystemExit("This isolated integration fixture requires Linux.")
    for tool in ("cargo", "nice", "ionice", "pipewire", "pw-dump", "pw-cli", "pw-link"):
        if shutil.which(tool) is None:
            raise SystemExit(f"Missing {tool}; no packages or system settings were changed.")
    fixture = Path(__file__).resolve().parent
    root = fixture.parent.parent
    work = Path(tempfile.mkdtemp(prefix="plugin-kit-private-audio-"))
    for name in ("runtime", "xdg", "state", "target"):
        (work / name).mkdir(mode=0o700)
    for name in ("pipewire.conf", "alsa.conf", "client.conf"):
        shutil.copyfile(fixture / name, work / name)
    shutil.copyfile(fixture / "client.conf", work / "client-rt.conf")
    # A unique config basename and explicit search directory prevent the normal
    # system/user pipewire.conf.d fragments from being inherited.
    (work / "pipewire.conf").rename(work / "standalone-private.conf")
    build_env = dict(os.environ, CARGO_BUILD_JOBS="4", CARGO_TARGET_DIR=str(work / "target"))
    # Compilation never initializes an audio host. Low-priority jobs preserve
    # the owner machine's interactive workload.
    subprocess.run(
        ["nice", "-n", "19", "ionice", "-c", "3", "cargo", "build", "-p",
         "plugin-kit-standalone", "--example", "private_device_probe"],
        cwd=root, env=build_env, check=True,
    )
    env = dict(os.environ, ALSA_CONFIG_PATH=str(work / "alsa.conf"),
               PIPEWIRE_RUNTIME_DIR=str(work / "runtime"), PIPEWIRE_REMOTE="ca72-private",
               PIPEWIRE_CONFIG_DIR=str(work),
               XDG_CONFIG_HOME=str(work / "xdg"), PLUGIN_KIT_PRIVATE_PIPEWIRE="1")
    (work / "environment.json").write_text(json.dumps({k: env[k] for k in (
        "ALSA_CONFIG_PATH", "PIPEWIRE_RUNTIME_DIR", "PIPEWIRE_REMOTE", "XDG_CONFIG_HOME")}, indent=2))
    evidence = []
    server = probe = None
    server_log = (work / "server.log").open("w")
    probe_log = (work / "probe.stderr").open("w")
    selector = selectors.DefaultSelector()

    def run(args):
        return subprocess.run(args, env=env, capture_output=True, timeout=2)

    def graph():
        result = run(["pw-dump"])
        if result.returncode:
            raise RuntimeError(result.stderr.decode().strip())
        objects = json.loads(result.stdout)
        # Any device or hardware-backed node invalidates the fixture's safety
        # claim. The configuration creates only a dummy driver and null nodes.
        for obj in objects:
            props = obj.get("info", {}).get("props", {})
            assert obj["type"] != "PipeWire:Interface:Device", obj
            assert "api.alsa.path" not in props and "device.api" not in props, obj
        return objects

    def start_server():
        child = subprocess.Popen(
            ["nice", "-n", "19", "pipewire", "-c", str(work / "standalone-private.conf")],
            env=env, stdout=server_log, stderr=server_log,
        )
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            if child.poll() is not None:
                raise RuntimeError("Private server failed; inspect server.log")
            if (work / "runtime" / "ca72-private").exists():
                return child
            time.sleep(.05)
        child.terminate()
        try:
            child.wait(timeout=3)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait(timeout=3)
        raise RuntimeError("Private server did not create its own socket")

    def connect_graph():
        objects = graph()
        nodes = {obj["id"]: obj.get("info", {}).get("props", {}).get("node.name", "")
                 for obj in objects if obj["type"] == "PipeWire:Interface:Node"}
        ports = {}
        existing = set()
        for obj in objects:
            info = obj.get("info", {})
            if obj["type"] == "PipeWire:Interface:Port":
                props = info["props"]
                ports.setdefault(int(props["node.id"]), {})[props["port.name"]] = obj["id"]
            elif obj["type"] == "PipeWire:Interface:Link":
                existing.add((info["output-port-id"], info["input-port-id"]))
        for node, name in nodes.items():
            if name.startswith(("alsa_playback.private_device_probe", "alsa_capture.private_device_probe")) and not ports.get(node):
                direction = "Output" if name.startswith("alsa_playback") else "Input"
                result = run(["pw-cli", "set-param", str(node), "PortConfig",
                              "{ direction=" + direction + " mode=dsp format={ mediaType=audio "
                              "mediaSubtype=raw format=F32P rate=48000 channels=2 position=[FL FR] } }"])
                if result.returncode:
                    raise RuntimeError(result.stderr.decode())
        by_name = {name: ports.get(node, {}) for node, name in nodes.items()}

        def link(source, source_port, dest, dest_port):
            pair = (by_name.get(source, {}).get(source_port), by_name.get(dest, {}).get(dest_port))
            if None not in pair and pair not in existing:
                result = run(["pw-link", str(pair[0]), str(pair[1])])
                if result.returncode:
                    raise RuntimeError(result.stderr.decode())

        for channel in ("FL", "FR"):
            link("PrivateSink", f"monitor_{channel}", "PrivateSource", f"input_{channel}")
            for name in by_name:
                if name.startswith("alsa_playback.private_device_probe"):
                    link(name, f"output_{channel}", "PrivateSink", f"playback_{channel}")
                if name.startswith("alsa_capture.private_device_probe"):
                    link("PrivateSource", f"capture_{channel}", name, f"input_{channel}")

    def receive():
        if not selector.select(8):
            raise RuntimeError("Probe control response timed out")
        line = probe.stdout.readline().strip()
        if not line:
            raise RuntimeError(f"Probe exited unexpectedly: {probe.poll()}")
        return line

    def request(command):
        probe.stdin.write(command + "\n")
        probe.stdin.flush()
        return receive()

    def status(label=None):
        value = json.loads(request("status"))
        if label:
            evidence.append(dict(label=label, **value))
            print(label, json.dumps(value), flush=True)
        return value

    def await_status(predicate, timeout=10, linking=True):
        deadline = time.monotonic() + timeout
        latest = None
        while time.monotonic() < deadline:
            if linking:
                connect_graph()
            latest = status()
            if predicate(latest):
                return latest
            time.sleep(.05)
        raise AssertionError(f"Timed out; last snapshot: {latest}")

    try:
        server = start_server()
        (work / "initial-graph.json").write_text(json.dumps(graph(), indent=2))
        probe = subprocess.Popen(
            [str(work / "target" / "debug" / "examples" / "private_device_probe"), str(work / "state")],
            env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=probe_log,
            text=True, bufsize=1,
        )
        selector.register(probe.stdout, selectors.EVENT_READ)
        assert receive() == "READY"
        await_status(lambda s: s["phase"] == "Running" and s["calls"] > 20)
        first = status("initial-playback")
        assert first["device_count"] == 1 and first["invalid"] == 0
        previous_input = 0
        for rate, period in ((48000, 128), (44100, 256), (48000, 256)):
            assert request(f"apply {rate} {period} on") == "APPLY Ok(())"
            await_status(lambda s: s["phase"] == "Running" and s["rate"] == rate
                         and s["input_nonzero"] > previous_input + 1000)
            current = status(f"duplex-{rate}-{period}")
            assert current["invalid"] == 0 and current["drops"] == 0
            assert current["requested_rate"] == rate and current["requested_period"] == period
            assert current["input_matches"] > current["input_nonzero"] * .95
            previous_input = current["input_nonzero"]
        # Kill only the server spawned here; no process-name signals are used.
        server.terminate()
        server.wait(timeout=5)
        lost = await_status(lambda s: s["phase"] in ("Recovering", "Error"), linking=False)
        status("server-disconnected")
        stopped_calls = lost["calls"]
        server = start_server()
        await_status(lambda s: s["phase"] == "Running" and s["calls"] > stopped_calls + 20)
        recovered = status("server-recovered")
        assert recovered["prepares"] > current["prepares"]
        assert recovered["drops"] == 0 and recovered["invalid"] == 0
        closed = request("close")
        final = json.loads(receive())
        evidence.append(dict(label="closed", close=closed, **final))
        print("closed", closed, json.dumps(final), flush=True)
        assert closed.startswith("CLOSED Ok(()) ") and int(closed.split()[-1][:-2]) < 2000
        assert final["phase"] == "Stopped" and final["drops"] == 1
        probe.wait(timeout=3)
        (work / "evidence.json").write_text(json.dumps(evidence, indent=2))
        print(f"PASS: evidence retained at {work}")
    finally:
        selector.close()
        for child in (probe, server):
            if child is not None and child.poll() is None:
                child.terminate()
                try:
                    child.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait(timeout=3)
        server_log.close()
        probe_log.close()
        (work / "evidence.json").write_text(json.dumps(evidence, indent=2))
        print(f"Fixture files: {work}", flush=True)


if __name__ == "__main__":
    main()
