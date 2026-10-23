# Librespot Connect 0.8.0 pause acknowledgement patch

Source: the crates.io librespot-connect 0.8.0 release, MIT license included.
The root Cargo.toml patches that exact version. Package versions and Cargo.lock
entries are otherwise unchanged.

Spirc changes its play status as soon as it sends play or pause to the player, then
applies the player's `Paused` event when it arrives. The player only reports a pause
after its audio sink has drained, so a pause followed quickly by play let that late
event switch Spirc back to `Paused` while audio kept playing. Spirc then published a
paused state, ignored the next play (it believed it was already paused) and loaded
skipped tracks paused.

This copy counts pauses Spirc sent while playing and drops their `Paused`
acknowledgement once a newer play has superseded them. A pause the player makes on
its own, such as after a decode error, still applies. Load requests reset the count
because the player's events for an older request are already ignored.
