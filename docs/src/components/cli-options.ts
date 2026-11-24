export interface CLIOption {
  flag: string;
  description: string;
}

/** Options available on every command. */
export const globalOptions: CLIOption[] = [
  {
    flag: "--config-file <path>",
    description: "Path to config file. Defaults to spawn.toml.",
  },
  { flag: "-d, --debug", description: "Turn on debug output." },
];

/** The --target flag. Relevant to commands that read or validate the target config. */
export const targetOption: CLIOption[] = [
  {
    flag: "--target <name>",
    description: "Select which target from spawn.toml to use.",
  },
];

/** The --environment flag used by migration subcommands. */
export const environmentOption: CLIOption[] = [
  {
    flag: "-e, --environment <name>",
    description: "Override the environment for the target config.",
  },
];

/** The --variables flag for loading template variables. Values are available in templates as `{{ variables.key }}`. */
export const variablesOption: CLIOption[] = [
  {
    flag: "--variables <path>",
    description:
      "Path to variables file (JSON, TOML, or YAML). Values are available in templates under {{ variables }}.",
  },
];

/** The pin flags shared by script run and script build, which pin automatically. */
export const scriptPinOptions: CLIOption[] = [
  {
    flag: "--pinned",
    description:
      "Require the pinned component tree. Fails if the script has not been pinned.",
  },
  {
    flag: "--no-pin",
    description:
      "Ignore the script's pin and resolve includes against the current components.",
  },
];

/** The repeatable --var-json flag for setting a single variable from the command line. */
export const varJsonOption: CLIOption[] = [
  {
    flag: "--var-json <KEY=JSON>",
    description:
      "Set a single variable from a JSON value, so it keeps its type. May be repeated, and overrides the same key from --variables. Keys may not contain '.', which is reserved for future nested overrides.",
  },
];

/** The --reveal-secrets flag used by build commands, which mask secret() values by default. */
export const revealSecretsOption: CLIOption[] = [
  {
    flag: "--reveal-secrets",
    description:
      "Render real secret() values instead of masked placeholders. For local debugging only — build output is not executed.",
  },
];
