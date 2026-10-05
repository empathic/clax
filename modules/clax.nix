self:
{ config, lib, pkgs, ... }:
let
  cfg = config.programs.clax;
  system = pkgs.stdenv.hostPlatform.system;
in
{
  options.programs.clax = {
    enable = lib.mkEnableOption "the clax CLI";

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${system}.clax;
      description = "The clax package to install (defaults to this flake's build).";
    };

    plugin = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = ''
        Link the Claude Code marketplace tree at
        `~/.config/claude/plugins/nix/clax`. That path is a directory
        marketplace; registering it and enabling `clax@clax` is the Claude
        settings' business (`extraKnownMarketplaces` and `enabledPlugins`),
        which this module does not write — and it is why `clax init` is not run
        for Claude Code. Keep `clax@clax` off human seats.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    home.packages = [ cfg.package ];
    home.file.".config/claude/plugins/nix/clax" = lib.mkIf cfg.plugin {
      source = self.packages.${system}.claude-plugin;
    };
  };
}
