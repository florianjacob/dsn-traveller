{ pkgs ? import <nixpkgs> {}, unstable ? import <nixos-unstable> {} }:
pkgs.mkShell {
  buildInputs = [
    # unstable.rustup
    # pin to specific version that works with current ruma dependencies,
    # I can probably pull this up as soon as the new ruma versions are released
    # (pkgs.rustChannelOf { date = "2018-05-14"; channel = "nightly";}).rust
    # (pkgs.rustChannelOf { date = "2018-12-31"; channel = "nightly";}).rust
    ((pkgs.rustChannelOf { date = "2019-01-31"; channel = "nightly";}).rust.override {
      extensions = [ "clippy-preview" "rustfmt-preview" ];
    })

    pkgs.openssl
    pkgs.pkgconfig
    pkgs.gcc
    pkgs.rr
  ];
}
