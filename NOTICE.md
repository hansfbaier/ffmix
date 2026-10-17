# Third-party notices and provenance

## Openface Mixer

The FireWire transport approach, FF800 encoding/configuration words and the mixer
model (send/pan/mute/solo folding, fader taper, meter scaling and channel layout) are
adapted from Openface Mixer:

- https://github.com/thomas-nelson23/Openface-Mixer
- Copyright (c) 2026 The Openface Mixer contributors
- MIT License

The MIT text applicable to those portions is the same as in `LICENSE`.

## RME / TotalMix FX

FFMix is an independent interoperability project. It is **not affiliated with,
sponsored by, or endorsed by RME**. "Fireface" and "TotalMix" are trademarks of RME.

No RME source code, firmware, binaries, installer contents, TotalMix FX artwork, or
other vendor materials are included in this repository or embedded in the application.
Register addresses, bit fields and coefficient encodings are hardware-interface facts;
the Rust implementation is original. Running FFMix does not require any RME software.

## Other references

The register behavior was understood from publicly documented interface behavior and
the Openface Mixer implementation. No source code from the Linux kernel, FFADO,
snd-firewire-ctl-services, or any other GPL project is included.
