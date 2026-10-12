# 3. Text actions use text-input-v3

The compositor delivers text through Wayland text-input-v3 commits. Synthetic key presses cannot express characters absent from the keymap, and an external input-method process adds an unnecessary dependency for declarative text actions. Tehuti handles text-input enablement and commit serials directly because Smithay's text-input helper requires an external input method.

Clients must support text-input-v3 and enable it for the focused surface. Actions without an enabled recipient are discarded; they are never redirected to another client or converted to keys. Focus changes clear enablement. Text commits use at most 4000 UTF-8 bytes each and preserve character boundaries and order. Empty text changes nothing. NUL is rejected at the configuration boundary because Wayland strings cannot encode it.

The automated delivery property states: for any generated Unicode string without NUL, including characters absent from the active keymap, the text action delivers exactly that string in order to the enabled focused client, with no modifiers or keys left pressed afterwards. The client runs over a socket pair, driven through SyntheticPad and input::route, without a display or devices.
