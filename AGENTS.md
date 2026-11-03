# XR system components

Maintain the independent Rust scene client, OS adapters and component package
here. Consume the Shell protocol SDK and Spatial Kit from their owners. Do not
put system calls on the Shell render thread or implement a second OpenXR
session. Probe actual services and expose unavailable capability/error states;
never substitute made-up values or successful action results.

Notifications may own org.freedesktop.Notifications only in an explicitly
isolated session bus. Do not replace the existing UKUI notification service,
modify the original desktop clipboard, or inject global input. Wayland
selection/seat/text-input belong to xr-wayland; lifecycle belongs to the
session manager. Installing packages must not start competing services.

Keep complete Git history and real contributor identity/dates. Public files
must not contain personal paths, LAN addresses, credentials or private device
identity. Cross-repository plans belong in openkylin-xr-docs.
