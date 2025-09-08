import { REST, Routes } from 'discord.js';
import config from './config/config';
import { CommandManager } from './utils/commandManager';
import 'dotenv/config';

const commandManager = new CommandManager();
await commandManager.loadCommands();

const commands = Array.from(commandManager.getCommands().values()).map(
  (command) => command.data.toJSON(),
);

const rest = new REST({ version: '10' }).setToken(process.env.DISCORD_TOKEN!);

(async () => {
  try {
    console.log(
      `Started refreshing ${commands.length} application (/) commands.`,
    );

    const data = await rest.put(
      Routes.applicationGuildCommands(config.clientId, config.guildId),
      { body: commands },
    );

    console.log(
      `Successfully reloaded application (/) commands. Registered ${commands.length} commands to guild ${config.guildId}`,
    );
  } catch (error) {
    console.error(error);
  }
})();
