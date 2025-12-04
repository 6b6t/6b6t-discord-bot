import * as fs from "node:fs";
import * as path from "node:path";
import { dirname } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { Collection } from "discord.js";
import type { Command } from "../types/command";

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

export class CommandManager {
  private commands: Collection<string, Command>;
  private cooldowns: Collection<string, Collection<string, number>>;

  constructor() {
    this.commands = new Collection();
    this.cooldowns = new Collection();
  }

  async loadCommands() {
    const commandsPath = path.join(__dirname, "../commands");
    const commandFiles = fs
      .readdirSync(commandsPath)
      .filter((file) => file.endsWith(".ts") || file.endsWith(".js"));

    for (const file of commandFiles) {
      const filePath = path.join(commandsPath, file);
      const fileUrl = pathToFileURL(filePath).href;
      const commandModule = await import(fileUrl);
      const command = commandModule.default;

      if (command && "data" in command && "execute" in command) {
        this.commands.set(command.data.name, command);
        console.log(`Loaded command: ${command.data.name}`);
      } else {
        console.log(`Failed to load command from file: ${file}`);
      }
    }
  }

  getCommands(): Collection<string, Command> {
    return this.commands;
  }

  getCommandsJSON() {
    return Array.from(this.commands.values()).map((command) =>
      command.data.toJSON(),
    );
  }

  isOnCooldown(
    commandName: string,
    userId: string,
    cooldownSeconds: number,
  ): number {
    let timestamps = this.cooldowns.get(commandName);
    if (!timestamps) {
      timestamps = new Collection();
      this.cooldowns.set(commandName, timestamps);
    }

    const now = Date.now();

    if (timestamps.has(userId)) {
      const expirationTime = timestamps.get(userId);
      if (!expirationTime) {
        return 0;
      }
      if (now < expirationTime) {
        return Math.ceil((expirationTime - now) / 1000);
      }
    }

    timestamps.set(userId, now + cooldownSeconds * 1000);
    return 0;
  }
}
