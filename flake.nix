{
  description = "clax — local artifacts with comment-driven development";

  inputs.nixpkgs.url = "github:nixos/nixpkgs/nixpkgs-unstable";

  outputs = { self, nixpkgs }:
    let
      systems = [ "aarch64-darwin" "x86_64-darwin" "aarch64-linux" "x86_64-linux" ];
      forAll = f: nixpkgs.lib.genAttrs systems (s: f nixpkgs.legacyPackages.${s});
    in
    {
      packages = forAll (pkgs: rec {
        # The shell/bridge bundle the server embeds (rust-embed folder
        # `web/dist/`). It is gitignored in the repo, so a source-only build
        # would embed an empty directory; build it here from web/'s lockfile.
        clax-web = pkgs.buildNpmPackage {
          pname = "clax-web";
          version = (pkgs.lib.importTOML ./Cargo.toml).workspace.package.version;
          src = ./web;
          npmDepsHash = "sha256-3vh0utMhfW5K3BfPaDswfs+KA026w1nven/dPVWAgOY=";
          # scripts/build-extension.mjs reads ../Cargo.toml for the version; src is web/ alone.
          postPatch = "cp ${./Cargo.toml} ../Cargo.toml";
          # `npm run build` is clean-dist + parts + bridge + shell; the shell
          # config writes to ../dist relative to web/shell, i.e. web/dist.
          installPhase = ''
            runHook preInstall
            cp -r dist $out
            runHook postInstall
          '';
        };

        clax = pkgs.rustPlatform.buildRustPackage {
          pname = "clax";
          version = (pkgs.lib.importTOML ./Cargo.toml).workspace.package.version;
          src = self;
          cargoLock.lockFile = ./Cargo.lock;
          cargoBuildFlags = [ "-p" "clax-cli" ];

          # rusqlite is `bundled` (needs a C compiler, which stdenv has); no
          # openssl: reqwest is default-features = false.
          postPatch = ''
            mkdir -p web/dist
            cp -r ${clax-web}/. web/dist/
            chmod -R u+w web/dist
          '';

          # The workspace's gates (`just ci`) run the tests, some of which
          # bind ports and spawn daemons; a package build does not.
          doCheck = false;

          meta = {
            description = "clax: local artifacts with comment-driven development";
            mainProgram = "clax";
          };
        };

        default = clax;

        # The Claude Code marketplace tree, laid out as the repo lays it out
        # (`.claude-plugin/marketplace.json` -> `./plugins/claude-code`), so
        # the manifest's relative source resolves. This is what the module
        # links; `clax init` is NOT needed for Claude Code when this is used.
        claude-plugin = pkgs.runCommandLocal "clax-claude-plugin" { } ''
          mkdir -p $out/.claude-plugin $out/plugins
          cp ${./.claude-plugin/marketplace.json} $out/.claude-plugin/marketplace.json
          cp -r ${./plugins/claude-code} $out/plugins/claude-code
        '';
      });

      homeManagerModules.clax = import ./modules/clax.nix self;
      homeManagerModules.default = self.homeManagerModules.clax;
    };
}
