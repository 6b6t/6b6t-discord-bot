import {
  Client,
  GatewayIntentBits,
  type Interaction,
  type Message,
  type MessageReaction,
  type PartialMessage,
  type PartialMessageReaction,
  Partials,
  type PartialUser,
  REST,
  Routes,
  type User,
} from "discord.js";
import config from "./config/config";
import { onReady } from "./events/ready";
import { CommandManager } from "./utils/commandManager";
import "dotenv/config";
import { onInteractionCreate } from "./events/interactionCreate";
import { onMessageCreate } from "./events/messageCreate";
import { onMessageReactionAdd } from "./events/messageReactionAdd";
import { onMessageReactionRemove } from "./events/messageReactionRemove";
import { onMessageUpdate } from "./events/messageUpdate";
import { closeAllMysqlPools } from "./utils/mysql-pool";

const client = new Client({
  allowedMentions: {
    // Prevent @everyone pings
    parse: ["users", "roles"],
  },
  intents: [
    GatewayIntentBits.Guilds,
    GatewayIntentBits.GuildMembers,
    GatewayIntentBits.GuildMessages,
    GatewayIntentBits.GuildMessageReactions,
  ],
  partials: [Partials.Message, Partials.Channel, Partials.Reaction],
});

const commandManager = new CommandManager();

async function initializeBot() {
  try {
    console.log("Loading commands...");
    await commandManager.loadCommands();

    const token = process.env.DISCORD_TOKEN;
    if (!token) {
      throw new Error("DISCORD_TOKEN is not set");
    }

    const rest = new REST({ version: "10" }).setToken(token);
    const commands = commandManager.getCommandsJSON();
    console.log(
      `Registering ${commands.length} commands to guild ${config.guildId}...`,
    );
    await rest.put(
      Routes.applicationGuildCommands(config.clientId, config.guildId),
      { body: commands },
    );

    console.log("Initializing bot...");
    await client.login(token);

    console.log(
      `Bot initialized and ready to serve in guild: ${config.guildId}`,
    );
  } catch (error) {
    console.error("Error initializing bot:", error);
    process.exit(1);
  }
}

client.once("clientReady", onReady);
client.on(
  "messageCreate",
  async (message: Message) => await onMessageCreate(client, message),
);
client.on(
  "messageUpdate",
  async (
    oldMessage: Message | PartialMessage,
    newMessage: Message | PartialMessage,
  ) => await onMessageUpdate(client, oldMessage, newMessage),
);
client.on(
  "interactionCreate",
  async (interaction: Interaction) =>
    await onInteractionCreate(commandManager, interaction),
);
client.on(
  "messageReactionAdd",
  async (
    reaction: MessageReaction | PartialMessageReaction,
    user: User | PartialUser,
  ) => await onMessageReactionAdd(client, reaction, user),
);
client.on(
  "messageReactionRemove",
  async (
    reaction: MessageReaction | PartialMessageReaction,
    user: User | PartialUser,
  ) => await onMessageReactionRemove(client, reaction, user),
);

async function gracefulShutdown() {
  console.log("Shutting down gracefully...");

  try {
    if (client.isReady()) {
      console.log("Logging out of Discord...");
      await client.destroy();
      await new Promise((resolve) => setTimeout(resolve, 1000));
    }
    console.log("Discord client destroyed");

    await closeAllMysqlPools();
    console.log("MySQL pools closed");

    console.log("Shutdown complete");
    process.exit(0);
  } catch (error) {
    console.error("Error during shutdown:", error);
    process.exit(1);
  }
}

process.on("SIGINT", () => {
  console.log("\nReceived SIGINT (Ctrl+C)");
  void gracefulShutdown();
});
process.on("SIGTERM", () => {
  console.log("\nReceived SIGTERM");
  void gracefulShutdown();
});
process.on("SIGUSR2", () => {
  console.log("\nReceived SIGUSR2");
  void gracefulShutdown();
});

process.on("uncaughtException", async (error) => {
  console.error("Uncaught Exception:", error);
});

process.on("unhandledRejection", async (reason, promise) => {
  console.error("Unhandled Rejection at:", promise, "reason:", reason);
});

void initializeBot();
