{ pkgs ? import <nixpkgs> {}, unstable ? import <nixos-unstable> {} }:
pkgs.mkShell {
  buildInputs = [
    # pin to specific version that works with current tokio async-await preview
    ((pkgs.rustChannelOf { date = "2019-04-11"; channel = "nightly";}).rust.override {
      extensions = [ "clippy-preview" "rustfmt-preview" ];
    })

    pkgs.openssl
    pkgs.pkgconfig
    pkgs.gcc
    pkgs.rr
  ];
}
