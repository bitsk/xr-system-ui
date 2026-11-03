# UKUI XR system components

An independent Rust scene client for the UKUI XR control centre, notifications
and spatial keyboard. Spatial Kit owns widgets; Shell owns rendering, focus and
workspaces; the Wayland service owns selection, seat and text input. This
component performs OS operations on its own worker, outside the render thread.
It opens no OpenXR session and package installation starts no services.
Widget actions use Kit interaction state: a button requires a matching captured
press/release inside its bounds, cancellation clears the gesture, and a slider
commits one final captured value rather than queuing OS writes on every drag.

## Spatial layout

The control centre is a compact independent panel. Its default page contains
application appearance, volume, actual network status and tool toggles. Detailed
IME/workspace state and operation results live on the optional details page.
Notifications, clipboard and keyboard open independently on demand; the keyboard
is hidden initially. They remain in the same independent component process and
use distinct absolute STAGE panel transforms, rather than a tall debug sheet.

| Panel | Logical pixels | Physical metres | STAGE centre | Rotation `[pitch,yaw,roll]` |
| --- | --- | --- | --- | --- |
| Control centre | 600 × 700 | .60 × .70 | `[1.18,1.60,-1.74]` | `[0,-18,0]` |
| Keyboard | 1000 × 280 | 1.35 × .378 | `[0,.60,-1.35]` | `[25,0,0]` |
| Notifications | 600 × 340 | .60 × .34 | `[0,2.38,-1.78]` | `[0,0,0]` |
| Clipboard | 600 × 300 | .60 × .30 | `[-1.18,1.60,-1.74]` | `[0,18,0]` |

Positive pitch means the keyboard faces upward in the Shell UI convention.
Against the main application baseline `[0,1.60,-1.80]`, final visual acceptance
must verify at least 32 logical pixels / .064 m of separation from the control
centre, using actual projected bounds. Source coordinates alone do not establish
that measurement. Shell owns independent GPU quads, local button depth and
matching 3D hit transforms.

Spatial glass always uses Kit's dark material: panel RGBA `(22,26,35,.78)`,
primary `#F2F6FC`, secondary `#94A3B8`, accent `#3790FA`; ordinary buttons use
white at 12% and hover at 24%, keyboard keycaps white at 15% with white labels.
The appearance buttons change the actual OS/application theme; they do not make
spatial glass light. Unavailable audio and absent Wi-Fi remain truthful states,
with diagnostics in details rather than invented device readings.

Shell `ComponentCommand` supports `control`, `keyboard`, and `notifications`
with `toggle`, `show`, and `hide`. The control centre's application button sends
`ComponentIntent` for Dock's launcher. Its own tool buttons open the independent
panels; copy/paste shortcuts send actual evdev Ctrl+C/V into the acknowledged
application focus. Explicit Unicode commits remain separate from IME preedit.
These visual changes retain every M2–M4 functionality and acceptance gate;
builds remain distinct from final rendered/input/OS acceptance.

Implemented adapters query actual services and report unavailable capabilities:

| Feature | Interface and acceptance boundary |
| --- | --- |
| UKUI theme | `org.ukui.style` through GSettings; light/dark write and readback |
| Volume/mute | Pulse API through `pactl`; PipeWire Pulse is supported; a virtual sink is labelled explicitly |
| Network | NetworkManager status through `nmcli`; no fabricated Wi-Fi device or network toggle |
| Notifications | Real `org.freedesktop.Notifications` D-Bus service, close/action signals and expiration; explicit private bus only |
| Clipboard | Shell/Wayland selection bridge; waits for actual selection update, never the original desktop clipboard |
| Workspaces | Shell snapshot and acknowledged `WorkspaceIntent`; no X11 workspace mutation |
| Keyboard | Linux evdev press/release through acknowledged Shell focus; literal Unicode/paste uses separate `TextCommit` |
| Chinese IME | Fcitx5 method discovery/switch/readback; Wayland service owns text-input/input-method/preedit |

The source builds with the versioned protocol SDK and Kit. Runtime and visual
acceptance must be recorded separately: source compilation does not establish
Chinese preedit, physical sound, or a rendered control interaction.

## Build

Use Rust 1.88 or later. Place the matching `xr-shell` and `xr-spatial-kit` source
repositories next to this repository, or install the matching source SDK Debian
packages. Third party crates must already be available in `CARGO_HOME`; builds
are offline and locked.

```sh
python3 tools/build.py
target/release/xr-system-ui --help
dpkg-buildpackage -b -us -uc
```

`XR_SHELL_PROTOCOLS_DIR` and `XR_SPATIAL_KIT_DIR` select an explicit source SDK.
The aggregate component lock must pin their Git revisions in addition to the
versions and Cargo lock here. SDKs are build dependencies; no Rust SDK is needed
on the target at runtime.

## Run and inspect

```sh
xr-system-ui probe
xr-system-ui run --socket "$XDG_RUNTIME_DIR/xr-desktop/shell.sock" \
  --state-file "$XDG_RUNTIME_DIR/xr-system-state.json"
xr-system-ui action volume 35
xr-system-ui action mute true
xr-system-ui action theme ukui-dark
xr-system-ui action workspace 1 --socket "$XDG_RUNTIME_DIR/xr-desktop/shell.sock"
xr-system-ui action clipboard 'UKUI XR' --socket "$XDG_RUNTIME_DIR/xr-desktop/shell.sock"
```

`--seconds N` bounds the scene client lifetime; SIGINT/SIGTERM also remove its
scene connection and release its owned notification name. `probe` never claims
a D-Bus name and never changes settings. Actions require the same private user,
bus and runtime environment as the selected development session. Successful
actions include readback; a missing endpoint or focus returns an error.
The scene client requests the full initial selection with `GetClipboard` and
stores full text only from `ClipboardChanged`; snapshot previews are never
treated as pasteable text. Shell's `SessionEnding` message is a normal exit;
an unannounced transport disconnect remains an error.

The session supervisor must establish its own user/session bus, configuration
directory, audio endpoint and Wayland display. A private audio stack can use
`pipewire`, `wireplumber`, `pipewire-pulse` and a private Pulse socket. An isolated
null sink can exercise volume control, but establishes no physical audio output.
Do not point the development session at the original UKUI user's Pulse socket.

To serve notifications, the supervisor must explicitly create a private D-Bus
session and set `XR_SYSTEM_UI_ISOLATED_BUS=1` before passing
`--isolated-notifications`. The service refuses an existing notification owner
or recognised desktop session manager, never requests replacement, and never
queues behind another owner. No D-Bus activation or login autostart is installed.

The supervisor starts Fcitx5 on the private bus and Wayland display. Switching
and probing require an existing D-Bus name owner; probes address its unique
connection name and do not activate an absent IME daemon. An inactive IME
profile therefore remains inactive when the control centre reads its state.
Switching
to `pinyin` and observing the method is only controller acceptance. Chinese
preedit/candidate/commit acceptance additionally requires a real text-input
client and the Wayland input-method path. The literal Chinese example button
tests Unicode commit separately and cannot establish Fcitx preedit support.

## Source and runtime requirements

`coreutils` supplies bounded adapter subprocesses. Optional adapter tools are
provided by `libglib2.0-bin`, `pulseaudio-utils`, `network-manager`, and `fcitx5`;
`fcitx5-chinese-addons` provides pinyin. Missing services remain visible as
unavailable. Nothing modifies global input, logs into the original desktop,
starts system services, or takes ownership of headset hardware.
