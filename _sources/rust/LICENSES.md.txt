# Cargo license overrides

The hatch-rs build hook uses `license-overrides.json` to supply license texts
that cargo-bundle-licenses 4.2.0 cannot discover. The file contains `r-efi`
6.0.0, `jni` 0.22.4, `jni-macros` 0.22.4, `jni-sys-macros` 0.4.1, and
`rustls-platform-verifier-android` 0.2.0, matched by package name, version, and
license identifier.
Other dependencies are collected from their source distributions at build time.

These overrides require human review before shipping. They do not certify
license compliance or change the crates' declared license choices.

## Sources

`r-efi` declares `MIT OR Apache-2.0 OR LGPL-2.1-or-later`. Its AUTHORS file
contains the MIT permission text, Apache and LGPL notices, copyright holders,
and contributor lists:

- [r-efi 6.0.0 AUTHORS](https://github.com/r-efi/r-efi/blob/7e1b0322d31d625f81a5656096330934f9cd835d/AUTHORS),
  SHA-256 `d027e91dbc9cdbb2f1190068e498bd6b61cff022b6a032b191021ba658d96111`.

The override preserves the complete AUTHORS text. The MIT entry
uses that text unchanged. The Apache entry appends the full Apache 2.0 text
from the repository's [LICENSE](../LICENSE). The LGPL entry appends the
[GNU LGPL 2.1 text](https://www.gnu.org/licenses/old-licenses/lgpl-2.1.txt);
the original notice retains the "or later" permission.

The jni crates and `rustls-platform-verifier-android` declare `MIT OR Apache-2.0`
and publish no license files. Their overrides use the repository's
`LICENSE-MIT` and `LICENSE-APACHE` at each published crate's VCS commit:

- [jni 0.22.4](https://github.com/jni-rs/jni-rs/tree/5ae9458a4ec44c5318f37ddc7569c1d4ae8a69e7)
  and [jni-macros 0.22.4](https://github.com/jni-rs/jni-rs/tree/33045a124105c939d1e2cbdcb5a39e5d868ffa03),
  SHA-256 `fea1d5bf3dd71605ce5d7d2ff695c1837e914c77195e523a86b5391716477960` (MIT) and
  `a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2` (Apache).
- [jni-sys-macros 0.4.1](https://github.com/jni-rs/jni-sys/tree/64d77b7a5f119d7b55b4e2c169a4668067ff59e6),
  SHA-256 `1d85bd754b04ceec93e98e890edd1fa3c6a22e81bcb32135806beeccefa51cd1` (MIT) and
  `c6596eb7be8581c18be736c846fb9173b69eccf6ef94c5135893ec56bd92ba08` (Apache).
- [rustls-platform-verifier-android 0.2.0](https://github.com/rustls/rustls-platform-verifier/tree/252e25161a91af476cbea620e29d277592d27ec7),
  SHA-256 `1c7cf76689c837a68ed8d704994e52a0f2940c087958f860d17f3186afbdcc0c` (MIT) and
  `c71d239df91726fc519c6eb72d318ec65820627232b2f796219e87dcf35d0ab4` (Apache).

The source distribution includes these overrides so wheels rebuilt from it
use the same inputs. The wheel includes the resulting bundle, not the override
source. If the collector cannot find license texts for a new crate version, the
build fails until its sources and notices are reviewed and added.
