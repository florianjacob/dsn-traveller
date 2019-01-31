# DSN Traveller #
## Travelling the Matrix network, for Science! ##
The DSN Traveller travels the [Matrix](https://matrix.org/) network
under the name of @dsn-traveller:dsn-traveller.dsn.scc.kit.edu and writes a travel report of what it saw. 📝

[More information](https://dsn-traveller.dsn.scc.kit.edu/)

## Requirements ##
This depends on rust nightly, I use it with rust nightly 2019-01-31,
and on a number of forks of the various [ruma](https://www.ruma.io/) crates with patches that I will upstream soon.

If you use NixOS, the provided `shell.nix` contains everything you need, which you can
[automatically load](https://nixos.wiki/wiki/Development_environment_with_nix-shell) via:
```
direnv allow .
```
