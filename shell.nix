{ pkgs ? import <nixpkgs> {}, unstable ? import <nixos-unstable> {} }:
pkgs.mkShell {
  buildInputs = [
    # unstable.rustup
    # pin to specific version that works with current ruma dependencies,
    # I can probably pull this up as soon as the new ruma versions are released
    (pkgs.rustChannelOf { date = "2018-05-14"; channel = "nightly";}).rust
    pkgs.openssl
    pkgs.pkgconfig
    pkgs.gcc
    pkgs.rr

    pkgs.python3
    pkgs.python3Packages.plumbum

    # TODO: upstream this?
    (pkgs.python3Packages.buildPythonPackage rec {
      pname = "networkit";
      version = "4.5";
      buildInputs = [
        pkgs.python3Packages.numpy
        pkgs.python3Packages.networkx
        pkgs.python3Packages.tabulate
        pkgs.python3Packages.seaborn
        pkgs.python3Packages.scikitlearn
        pkgs.python3Packages.ipython
      ];
      propagatedBuildInputs = [
        pkgs.python3Packages.scipy
        pkgs.python3Packages.pandas
        pkgs.python3Packages.matplotlib
      ];
      src = pkgs.python3Packages.fetchPypi {
        inherit pname version;
        sha256 = "d2f7862970d376da916a4f87bf6d90e080dcbb37ab74a30dc6b3924b3b7b0475";
      };
      doCheck = false;
    })
  ];
}
