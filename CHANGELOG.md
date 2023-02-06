# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0] 2023-02-06

### Add functionality

- Implement abort handling ([#5][])

### Breaking changes

- Make internal implementation details private ([#2][])
- Remove `'static'` lifetime requirement of the USB bus ([#4][])

### Changes

- Fix formatting ([#3][])
- Use `assert!` in const instead of a inlined `const_assert!` ([#7][])
- Fix clippy warnings ([#8][])

### Bugfixes

- Upstreamed changes from the Nitrokey repository ([#1][])
  - Fix panic on 64 bit targets
  - Fix incorrect length check with the `highspeed-usb` feature
- Fix typo in calcualtion of packet lengths ([#6][])

[#1]: https://github.com/trussed-dev/usbd-ccid/pull/1
[#2]: https://github.com/trussed-dev/usbd-ccid/pull/2
[#3]: https://github.com/trussed-dev/usbd-ccid/pull/3
[#4]: https://github.com/trussed-dev/usbd-ccid/pull/4
[#5]: https://github.com/trussed-dev/usbd-ccid/pull/5
[#6]: https://github.com/trussed-dev/usbd-ccid/pull/6
[#7]: https://github.com/trussed-dev/usbd-ccid/pull/7
[#8]: https://github.com/trussed-dev/usbd-ccid/pull/8


## [0.1.0] 2023-24-01

[Unreleased]: https://github.com/trussed-dev/usbd-ccid/compare/0.2.0...HEAD
[0.2.0]: https://github.com/trussed-dev/usbd-ccid/releases/tag/0.2.0
[0.1.0]: https://github.com/trussed-dev/usbd-ccid/releases/tag/0.1.0
