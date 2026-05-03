# Contributing

Thanks for considering a contribution to `lumencast-rs`.

## Where to file what

| You want to                       | Go to                                                                 |
| --------------------------------- | --------------------------------------------------------------------- |
| Change the protocol or schema     | [`lumencast-protocol`](https://github.com/Lumencast/lumencast-protocol) — RFC process |
| Fix a bug in the Rust SDK         | This repo: open an issue, then a PR                                   |
| Add a feature to the Rust SDK     | This repo: open an issue first to discuss design                      |
| Improve documentation             | This repo: open a PR directly                                         |

Protocol or schema changes MUST land in `lumencast-protocol` first; this
SDK only tracks the spec.

## Local checks

Before pushing, run:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-features
cargo doc --workspace --no-deps
```

CI runs the same on Linux, macOS, and Windows.

## Conformance

Any change to the protocol crate MUST keep the conformance scenarios
green:

```sh
cargo test -p lumencast-conformance --release
```

If you change behaviour that the suite does not yet cover, add a
scenario upstream in `lumencast-protocol/conformance/` first.

## Commits

- One logical change per commit.
- Commit messages in English, present tense (`add`, `fix`, `refactor`).
- Reference issue numbers when relevant.

## License of contributions

By contributing you agree that your work is licensed under Apache-2.0
matching the project license. Sign-offs are not required.
