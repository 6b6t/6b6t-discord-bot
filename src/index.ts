import {Client, GatewayIntentBits, Message, REST, Routes} from 'discord.js';
import config from './config/config';
import {onReady} from './events/ready';
import {CommandManager} from './utils/commandManager';
import 'dotenv/config'
import {getRedisClient} from "./utils/redis";
import { onMessageCreate } from './events/messageCreate';

const client = new Client({
  intents: [
    GatewayIntentBits.Guilds,
    GatewayIntentBits.GuildMembers,
    GatewayIntentBits.GuildMessages,
  ]
});

const commandManager = new CommandManager();

async function initializeBot() {
  try {
    console.log('Loading commands...');
    commandManager.loadCommands();

    const rest = new REST({version: '10'}).setToken(process.env.DISCORD_TOKEN!);
    const commands = commandManager.getCommandsJSON();
    console.log(`Registering ${commands.length} commands to guild ${config.guildId}...`);
    await rest.put(
        Routes.applicationGuildCommands(config.clientId, config.guildId),
        {body: commands}
    );

    console.log('Initializing bot...');
    await client.login(process.env.DISCORD_TOKEN!);

    console.log(`Bot initialized and ready to serve in guild: ${config.guildId}`);
  } catch (error) {
    console.error('Error initializing bot:', error);
    process.exit(1);
  }
}

client.once('ready', onReady);
client.on('messageCreate', async (message: Message) => await onMessageCreate(client, message));

async function gracefulShutdown() {
  console.log('Shutting down gracefully...');

  try {
    (await getRedisClient()).disconnect();
    console.log('Redis connections closed');

    if (client.isReady()) {
      console.log('Logging out of Discord...');
      await client.destroy();
      await new Promise(resolve => setTimeout(resolve, 1000));
    }
    console.log('Discord client destroyed');

    console.log('Shutdown complete');
    process.exit(0);
  } catch (error) {
    console.error('Error during shutdown:', error);
    process.exit(1);
  }
}

process.on('SIGINT', () => {
  console.log('\nReceived SIGINT (Ctrl+C)');
  void gracefulShutdown();
});
process.on('SIGTERM', () => {
  console.log('\nReceived SIGTERM');
  void gracefulShutdown();
});
process.on('SIGUSR2', () => {
  console.log('\nReceived SIGUSR2');
  void gracefulShutdown();
});

process.on('uncaughtException', async (error) => {
  console.error('Uncaught Exception:', error);
});

process.on('unhandledRejection', async (reason, promise) => {
  console.error('Unhandled Rejection at:', promise, 'reason:', reason);
});

void initializeBot();
