# Cursor themes

`cua.default.lottie` is a file of Cua Driver, unchanged: the source of its
built-in cursor theme, from `libs/cua-driver/rust/crates/cursor-overlay/assets/`
of https://github.com/trycua/cua at the tag `cua-driver-rs-v0.33.1`
(SHA-256 `322e40a33475599b4403a6cb2ad3286d3312a6f9c71d94abfc92dbca8fa99de8`).
That repository's `LICENSING.md` puts Cua Driver under the root MIT License,
and the theme names MIT as its own licence (`cua/theme.json` inside the
archive).

The `ai.snth.connect.*.cua-theme` files are made from it by
`scripts/build-cursor-themes.py`: the same drawings with the one palette
colour replaced and our id, compiled with Cua Driver's `cua-cursor-theme`
0.33.1. They are compiled into SNTH Connect and written into the app's own
driver home.

```
MIT License

Copyright (c) 2025 Cua AI, Inc.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```
