# thepia

The Thepia command line (`the`) on npm. Inspect, validate, and refine agent skills across Claude Code,
Gemini CLI, OpenCode, and Goose.

```sh
npx thepia skills lint      # one-off, no install
npm i -g thepia             # install the `the` / `thepia` / `th` commands
```

`thepia`, `the`, and `th` are the same command; use whichever you prefer.

## How it installs

This package ships no binary itself. The prebuilt native binary for your machine arrives as one of the
per-platform packages listed in `optionalDependencies` (`@thepia/cli-darwin-arm64`,
`@thepia/cli-darwin-x64`, `@thepia/cli-linux-x64`, `@thepia/cli-linux-arm64`). npm installs only the one
whose `os`/`cpu` match this machine, so you download just your platform's binary. Everything arrives over
the npm registry channel — there is no CDN download and no postinstall network call, so it works behind
registry-proxying corporate firewalls (see ADR-0004).

Platforms without a prebuilt binary (Windows, Linux on musl/Alpine, other CPUs) print a short message
pointing at the `cargo` / Nix install paths.

## Other install channels

```sh
cargo install --git https://github.com/Thepia/the thepia
nix run github:Thepia/thepia -- --help
```

## License

MIT OR Apache-2.0.
