# Cargo license overrides

The hatch-rs build hook uses `license-overrides.json` to fill license texts
that cargo-bundle-licenses 4.2.0 cannot discover. The file contains only
`r-efi` 5.3.0 and 6.0.0, matched by package name, version, and license identifier.
Other dependencies are collected from their source distributions at build time.

These overrides require human review before shipping. They do not certify
license compliance or change the crates' declared license choices.

## Sources

Both versions declare `MIT OR Apache-2.0 OR LGPL-2.1-or-later`. Their AUTHORS
files contain the MIT permission text, Apache and LGPL notices, copyright
holders, and contributor lists:

- [r-efi 5.3.0 AUTHORS](https://github.com/r-efi/r-efi/blob/97b55bed1c2c91dcbf787674849f05337ff80b33/AUTHORS),
  SHA-256 `ff92bed461f50338dd703a9ba9aee496a425957873df1d91192776e4bdf5dda7`.
- [r-efi 6.0.0 AUTHORS](https://github.com/r-efi/r-efi/blob/7e1b0322d31d625f81a5656096330934f9cd835d/AUTHORS),
  SHA-256 `d027e91dbc9cdbb2f1190068e498bd6b61cff022b6a032b191021ba658d96111`.

Each override preserves its version's complete AUTHORS text. The MIT entry
uses that text unchanged. The Apache entry appends the full Apache 2.0 text
from the repository's [LICENSE](../LICENSE). The LGPL entry appends the
[GNU LGPL 2.1 text](https://www.gnu.org/licenses/old-licenses/lgpl-2.1.txt);
the original notice retains the "or later" permission.

The source distribution includes these overrides so wheels rebuilt from it
use the same inputs. The wheel includes the resulting bundle, not the override
source. New crate versions with undiscovered texts fail the build until their
sources and notices are reviewed and added.
