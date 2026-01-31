export interface CLIOption {
  flag: string;
  description: string;
}

/** Options available on every command. */
export const globalOptions: CLIOption[] = [
  { flag: "--config-file <path>", description: "Path to config file. Defaults to spawn.toml." },
  { flag: "-d, --debug", description: "Turn on debug output." },
];

/** The --database flag. Relevant to commands that read or validate the database config. */
export const databaseOption: CLIOption[] = [
  { flag: "--database <name>", description: "Select which database from spawn.toml to use." },
];

/** The --environment flag used by migration subcommands. */
export const environmentOption: CLIOption[] = [
  { flag: "-e, --environment <name>", description: "Override the environment for the database config." },
];
