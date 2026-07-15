# Changelog

All notable changes to this project will be documented in this file. See [conventional commits](https://www.conventionalcommits.org/) for commit guidelines.

---
## [0.1.1](https://github.com/zlx2019/deskmate/compare/v0.1.0..v0.1.1) - 2026-07-15

### Bug Fixes

- **(core)** report macOS by its official name - ([56c5171](https://github.com/zlx2019/deskmate/commit/56c51716e706fc10c69d633697b00a97ecc541d1)) - Zero
- **(desktop)** shut down gracefully on SIGINT/SIGTERM - ([940f058](https://github.com/zlx2019/deskmate/commit/940f0584dc6b1f735e8b9e187143c7eceb3d1aaa)) - Zero
- **(discovery)** keep mdns-discovered peers alive without UDP heartbeats - ([db74d62](https://github.com/zlx2019/deskmate/commit/db74d624516c5c63aa577845f35d04ed83cf3216)) - Zero
- **(ui)** drop hints from passive mode and autostart toggles - ([acb6e52](https://github.com/zlx2019/deskmate/commit/acb6e520c65904a32a5e4527009ce3bfa6cc90e0)) - Zero
- **(ui)** replace bulky delete buttons with hover corner badges - ([3baa805](https://github.com/zlx2019/deskmate/commit/3baa80507fce215b48e6a7ab583b1c6dff3b8784)) - Zero
- **(ui)** split peer detail line into OS and IP rows - ([9c3cf49](https://github.com/zlx2019/deskmate/commit/9c3cf49f9c518a4f9f5773ff61e5aa427da63d60)) - Zero
- harden transfer engine and UI against code-review findings - ([7030a8e](https://github.com/zlx2019/deskmate/commit/7030a8e6881336c0cd7abb8edbcf635369705f74)) - Zero

### Features

- **(desktop)** auto-copy received text to clipboard (opt-in) - ([f988993](https://github.com/zlx2019/deskmate/commit/f98899315955acd1113eb4d9397aa3f89cc9fed0)) - Zero
- **(desktop)** send clipboard screenshots as files - ([e8feac7](https://github.com/zlx2019/deskmate/commit/e8feac776c8ba32db5ddea0404a9773425cf29a3)) - Zero
- **(ui)** add history tab and chat message composer in transfer panel - ([5a0d8c0](https://github.com/zlx2019/deskmate/commit/5a0d8c0cf988fc4c955f56f653858644718e5179)) - Zero
- **(ui)** delete and clear for transfer history and text messages - ([176c9d1](https://github.com/zlx2019/deskmate/commit/176c9d149ace3d7ee04cc504672f934bb4d4f63d)) - Zero
- **(ui)** paste screenshot in message composer to send it - ([6911cc2](https://github.com/zlx2019/deskmate/commit/6911cc2035ea01d7ab925c0772fa242e51a6203e)) - Zero
- broadcast OS version and clean up radar bubble - ([b8f096f](https://github.com/zlx2019/deskmate/commit/b8f096f60fccb7c31ccbd42e380dfec27e249864)) - Zero
- i18n with Chinese and English UI languages - ([a14d253](https://github.com/zlx2019/deskmate/commit/a14d253864d1e5c8e17fff2ebc97343e5cb899ad)) - Zero

### Miscellaneous Chores

- **(ci)** version upgrade - ([8e39991](https://github.com/zlx2019/deskmate/commit/8e39991c5723241a02dcc2ca32816b3a5e9f2142)) - Zero
- **(deps)** sync pnpm lockfile with upgraded frontend toolchain - ([d30562c](https://github.com/zlx2019/deskmate/commit/d30562c1d144acc98d937dbf74a7ed40dffd82b4)) - Zero
- edit comment - ([b45e704](https://github.com/zlx2019/deskmate/commit/b45e7049fe7ba434d3507b8e4c1fd4f8b2f7487f)) - Zero

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

### Performance

- **(core)** socket buffers, file preallocation and F_NOCACHE - ([f94fc9f](https://github.com/zlx2019/deskmate/commit/f94fc9f5561709feadb41f9efe01fe0987d2d8a5)) - Zero

### Refactoring

- **(desktop)** harden screenshot pipeline and split commands module - ([5cec979](https://github.com/zlx2019/deskmate/commit/5cec979fa139b5dabf72416448189e5b62811749)) - Zero

---
## [0.1.0] - 2026-07-14

### Documentation

- **(core)** fix intra-doc link to private item - ([7bad3c6](https://github.com/zlx2019/deskmate/commit/7bad3c602124b173c6199f59f4c95532b6d34dc0)) - Zero

### Features

- releases v0.1.0 - ([828e1b7](https://github.com/zlx2019/deskmate/commit/828e1b7884d8fc11cd8357909ca9d355bdb5fd49)) - Zero

<!-- generated by git-cliff -->
