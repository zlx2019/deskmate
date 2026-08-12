# Changelog

All notable changes to this project will be documented in this file. See [conventional commits](https://www.conventionalcommits.org/) for commit guidelines.

---
## [0.1.0] - 2026-08-04

### Bug Fixes

- **(ci)** pass changelog through to tauri-action release updates - ([a3aa25f](https://github.com/zlx2019/deskmate/commit/a3aa25fe32bdb08e32329f1ea974406a9985623f)) - Zero
- **(core)** report macOS by its official name - ([56c5171](https://github.com/zlx2019/deskmate/commit/56c51716e706fc10c69d633697b00a97ecc541d1)) - Zero
- **(desktop)** shut down gracefully on SIGINT/SIGTERM - ([940f058](https://github.com/zlx2019/deskmate/commit/940f0584dc6b1f735e8b9e187143c7eceb3d1aaa)) - Zero
- **(discovery)** keep mdns-discovered peers alive without UDP heartbeats - ([db74d62](https://github.com/zlx2019/deskmate/commit/db74d624516c5c63aa577845f35d04ed83cf3216)) - Zero
- **(discovery)** defer probe-failure removal to mDNS cache verification - ([01a7f85](https://github.com/zlx2019/deskmate/commit/01a7f8521c8359d704aac16ae52a2b276d568281)) - Zero
- **(discovery)** rebuild multicast membership after silent loss - ([57ca601](https://github.com/zlx2019/deskmate/commit/57ca6019d5e8afe0da3cec0a57f8898f638cefbd)) - Zero
- **(discovery)** require a fingerprint match before a probe counts as alive - ([9649dbd](https://github.com/zlx2019/deskmate/commit/9649dbd09ac3449220c9f6762345f56fc883b0b1)) - Zero
- **(ui)** drop hints from passive mode and autostart toggles - ([acb6e52](https://github.com/zlx2019/deskmate/commit/acb6e520c65904a32a5e4527009ce3bfa6cc90e0)) - Zero
- **(ui)** replace bulky delete buttons with hover corner badges - ([3baa805](https://github.com/zlx2019/deskmate/commit/3baa80507fce215b48e6a7ab583b1c6dff3b8784)) - Zero
- **(ui)** split peer detail line into OS and IP rows - ([9c3cf49](https://github.com/zlx2019/deskmate/commit/9c3cf49f9c518a4f9f5773ff61e5aa427da63d60)) - Zero
- **(ui)** move composer send-error tip into its own banner - ([86fcb4b](https://github.com/zlx2019/deskmate/commit/86fcb4be960f2f73f8a3c49282821d39460712d4)) - Zero
- harden transfer engine and UI against code-review findings - ([7030a8e](https://github.com/zlx2019/deskmate/commit/7030a8e6881336c0cd7abb8edbcf635369705f74)) - Zero

### Documentation

- **(core)** fix intra-doc link to private item - ([7bad3c6](https://github.com/zlx2019/deskmate/commit/7bad3c602124b173c6199f59f4c95532b6d34dc0)) - Zero

### Features

- **(desktop)** auto-copy received text to clipboard (opt-in) - ([f988993](https://github.com/zlx2019/deskmate/commit/f98899315955acd1113eb4d9397aa3f89cc9fed0)) - Zero
- **(desktop)** send clipboard screenshots as files - ([e8feac7](https://github.com/zlx2019/deskmate/commit/e8feac776c8ba32db5ddea0404a9773425cf29a3)) - Zero
- **(discovery)** hot-toggle stealth mode without restart - ([4670da1](https://github.com/zlx2019/deskmate/commit/4670da1bb895c78a4e2c556c2e7a82733e9ece42)) - Zero
- **(discovery)** probe suspect peers over TCP to detect crashes fast - ([2a8154f](https://github.com/zlx2019/deskmate/commit/2a8154f1a87850466299b2162941d4bf141f86be)) - Zero
- **(transfer)** propagate pause/resume/cancel to the peer in real time - ([00bf178](https://github.com/zlx2019/deskmate/commit/00bf178ebb409464cfd1a4586565aeea46b2fec9)) - Zero
- **(ui)** add history tab and chat message composer in transfer panel - ([5a0d8c0](https://github.com/zlx2019/deskmate/commit/5a0d8c0cf988fc4c955f56f653858644718e5179)) - Zero
- **(ui)** delete and clear for transfer history and text messages - ([176c9d1](https://github.com/zlx2019/deskmate/commit/176c9d149ace3d7ee04cc504672f934bb4d4f63d)) - Zero
- **(ui)** paste screenshot in message composer to send it - ([6911cc2](https://github.com/zlx2019/deskmate/commit/6911cc2035ea01d7ab925c0772fa242e51a6203e)) - Zero
- **(ui)** follow the system theme by default - ([08b7969](https://github.com/zlx2019/deskmate/commit/08b79691f1801fd280c5524383bd5a6fdda37560)) - Zero
- releases v0.1.0 - ([828e1b7](https://github.com/zlx2019/deskmate/commit/828e1b7884d8fc11cd8357909ca9d355bdb5fd49)) - Zero
- broadcast OS version and clean up radar bubble - ([b8f096f](https://github.com/zlx2019/deskmate/commit/b8f096f60fccb7c31ccbd42e380dfec27e249864)) - Zero
- i18n with Chinese and English UI languages - ([a14d253](https://github.com/zlx2019/deskmate/commit/a14d253864d1e5c8e17fff2ebc97343e5cb899ad)) - Zero
- localize backend errors via stable error codes - ([9f95419](https://github.com/zlx2019/deskmate/commit/9f95419917de1b2b7a96ec26b7ac2416aec4f1d0)) - Zero

### Miscellaneous Chores

- **(ci)** version upgrade - ([8e39991](https://github.com/zlx2019/deskmate/commit/8e39991c5723241a02dcc2ca32816b3a5e9f2142)) - Zero
- **(deps)** sync pnpm lockfile with upgraded frontend toolchain - ([d30562c](https://github.com/zlx2019/deskmate/commit/d30562c1d144acc98d937dbf74a7ed40dffd82b4)) - Zero
- edit comment - ([b45e704](https://github.com/zlx2019/deskmate/commit/b45e7049fe7ba434d3507b8e4c1fd4f8b2f7487f)) - Zero
- add CHANGELOG.md - ([160d7b4](https://github.com/zlx2019/deskmate/commit/160d7b45bd5abf071b02e77ada10fa557a279367)) - Zero

### Other

- Merge pull request #5 from zlx2019/feat/transfer-panel-ui

feat(ui): add history tab and chat message composer in transfer panel - ([ec9aa89](https://github.com/zlx2019/deskmate/commit/ec9aa893dfbd1862708f345b3438d5ddb6a06147)) - Zero
- Merge pull request #6 from zlx2019/fix/mdns-peer-liveness

fix(discovery): keep mDNS-discovered peers alive without UDP heartbeats - ([ade3c4a](https://github.com/zlx2019/deskmate/commit/ade3c4a833d03dab2ffe298b71ab4e0f6754771e)) - Zero
- Merge pull request #7 from zlx2019/fix/mdns-peer-liveness

fix(desktop): shut down gracefully on SIGINT/SIGTERM - ([658dd46](https://github.com/zlx2019/deskmate/commit/658dd4632795b4d318e6eae23ebd17ac1b771fe9)) - Zero
- Merge pull request #8 from zlx2019/feat/auto-copy-text

feat(desktop): auto-copy received text to clipboard (opt-in) - ([2eb86aa](https://github.com/zlx2019/deskmate/commit/2eb86aa36d2190acb312b6b4361ca410ec37cffe)) - Zero
- Merge pull request #9 from zlx2019/feat/clear-records

feat(ui): delete and clear for transfer history and text messages - ([c89173a](https://github.com/zlx2019/deskmate/commit/c89173a873b2f118685aa31ffebb16bb0272468a)) - Zero
- Merge pull request #10 from zlx2019/feat/hotkey-settings-tabs

feat(desktop): global hotkey to send clipboard and tabbed settings - ([4ec6f2e](https://github.com/zlx2019/deskmate/commit/4ec6f2ea1f06caa7ce5023ff3d70cd3321581bfb)) - Zero
- land PR #10 content onto main - ([faac6e1](https://github.com/zlx2019/deskmate/commit/faac6e12be35ce8ea82a9c076cc00a73f706a8e0)) - Zero
- Merge pull request #11 from zlx2019/fix/land-pr10

merge: land PR #10 content onto main - ([755ca16](https://github.com/zlx2019/deskmate/commit/755ca169e47a58fc62e2b980f2f323d5cc607d17)) - Zero
- Merge pull request #12 from zlx2019/feat/notify-copied

feat(desktop): mark text notifications as copied when auto-copy is on - ([8077f50](https://github.com/zlx2019/deskmate/commit/8077f50069f0bea86b8d069b84556d520550d618)) - Zero
- Merge pull request #13 from zlx2019/feat/peer-os-version

feat: broadcast OS version and clean up radar bubble - ([5a2e372](https://github.com/zlx2019/deskmate/commit/5a2e372a5fe3ad5bb6339e1aabd63d776fd8137d)) - Zero
- Merge pull request #14 from zlx2019/fix/peer-detail-rows

fix(ui): split peer detail line into OS and IP rows - ([2f9b4ad](https://github.com/zlx2019/deskmate/commit/2f9b4ad1fdc3595849a906350c59dee9e8c6c2a3)) - Zero
- Merge pull request #15 from zlx2019/refactor/review-fixes

refactor(desktop): harden screenshot pipeline and split commands module - ([f8f18bc](https://github.com/zlx2019/deskmate/commit/f8f18bc511abb01f59f7b536eebca5c7ce4d46f5)) - Zero
- Merge pull request #16 from zlx2019/fix/code-review-issues

fix: harden transfer engine and UI against code-review findings - ([0e04abb](https://github.com/zlx2019/deskmate/commit/0e04abbc3664cd3b139f638d04db25739a05680e)) - Zero
- Merge pull request #17 from zlx2019/feat/i18n

feat: i18n with Chinese and English UI languages - ([90b907d](https://github.com/zlx2019/deskmate/commit/90b907d2d44e8561f3cb64a3593572237a13f1c3)) - Zero
- Merge pull request #18 from zlx2019/perf/io-tuning

perf(core): socket buffers, file preallocation and F_NOCACHE - ([bfbaccf](https://github.com/zlx2019/deskmate/commit/bfbaccf32f29a24d67129ce739c0a49f78c47f6f)) - Zero
- Merge pull request #19 from zlx2019/feat/pause-visibility-stealth-toggle

feat: real-time pause visibility and stealth hot-toggle - ([74d52f6](https://github.com/zlx2019/deskmate/commit/74d52f67ea4444f8f0c81f10e0a2a79f6dc26daa)) - Zero
- Merge pull request #20 from zlx2019/feat/peer-liveness-probe

feat: TCP liveness probe for crashed mDNS-only peers - ([65fa2a4](https://github.com/zlx2019/deskmate/commit/65fa2a4c772c199ab8ed86332be7a5bb619515e6)) - Zero
- Merge pull request #21 from zlx2019/fix/probe-mdns-cache-coherency

fix: one-way peer visibility after short network outage - ([69e9538](https://github.com/zlx2019/deskmate/commit/69e9538f0f30465ece191375ae34fd132dfef4af)) - Zero
- Merge pull request #22 from zlx2019/feat/system-theme-and-tip-banner

feat: system theme following and composer error banner - ([e395999](https://github.com/zlx2019/deskmate/commit/e395999c0baf3b8437f29e529af8a76efc8336de)) - Zero
- Merge pull request #24 from zlx2019/fix/land-error-codes

fix: land PR #23 (error-code i18n) onto main - ([2353afb](https://github.com/zlx2019/deskmate/commit/2353afb51e184b3a960be75af1fb44b8e79c78b8)) - Zero
- Merge pull request #25 from zlx2019/fix/udp-multicast-rejoin

fix: multicast membership self-heal and release changelog passthrough - ([884118d](https://github.com/zlx2019/deskmate/commit/884118db98e16d7e8eb9c36e8d9fbc1291df9ff3)) - Zero
- Merge pull request #36 from zlx2019/fix/probe-identity-check

fix(discovery): require a fingerprint match before a probe counts as alive - ([fd8397d](https://github.com/zlx2019/deskmate/commit/fd8397dbe9999596a90f462bebacb2f43aaa46e7)) - Zero

### Performance

- **(core)** socket buffers, file preallocation and F_NOCACHE - ([f94fc9f](https://github.com/zlx2019/deskmate/commit/f94fc9f5561709feadb41f9efe01fe0987d2d8a5)) - Zero

### Refactoring

- **(desktop)** harden screenshot pipeline and split commands module - ([5cec979](https://github.com/zlx2019/deskmate/commit/5cec979fa139b5dabf72416448189e5b62811749)) - Zero

<!-- generated by git-cliff -->
