# Nix

This repository exports a Nix flake for installing Kern through Nix
configuration and entering a development shell.

## Install Kern

Add this repository as a flake input and apply its overlay:

```nix
{
  inputs.kern.url = "github:kern-project/kern";

  outputs = { nixpkgs, kern, ... }: {
    nixosConfigurations.host = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [
        {
          nixpkgs.overlays = [ kern.overlays.default ];
        }
      ];
    };
  };
}
```

After that, install the packages from `pkgs` in your configuration.

NixOS:

```nix
environment.systemPackages = with pkgs; [
  kern
];
```

Home Manager:

```nix
home.packages = with pkgs; [
  kern
];
```

If you only want individual tools, use:

```nix
environment.systemPackages = with pkgs; [
  kernc
  craft
  kern-lsp
];
```

## Development Shell

Enter the development environment:

```sh
nix develop
```

The shell provides Rust, LLVM 21, `clang`, `lld`, and the environment variables
needed for this workspace's `llvm-sys` integration.
