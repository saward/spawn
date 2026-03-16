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
