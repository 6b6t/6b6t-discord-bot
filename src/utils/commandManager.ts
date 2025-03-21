import { Collection } from 'discord.js';
import { Command } from '../types/command';
import * as fs from 'fs';
import * as path from 'path';

export class CommandManager {
    private commands: Collection<string, Command>;

    constructor() {
        this.commands = new Collection();
    }

    loadCommands() {
        const commandsPath = path.dirname("../commands");
        const commandFiles = fs.readdirSync(commandsPath).filter(file => file.endsWith('.ts') || file.endsWith('.js'));

        for (const file of commandFiles) {
            const filePath = path.join(commandsPath, file);
            const commandModule = require(filePath);
            const command = commandModule.default;

            if (command && 'data' in command && 'execute' in command) {
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
        return Array.from(this.commands.values()).map(command => command.data.toJSON());
    }
}
