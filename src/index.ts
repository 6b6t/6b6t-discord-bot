import "dotenv/config";
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
import { onInteractionCreate } from "./events/interactionCreate";
import { onMessageCreate } from "./events/messageCreate";
import { onMessageReactionAdd } from "./events/messageReactionAdd";
import { onMessageReactionRemove } from "./events/messageReactionRemove";
import { onMessageUpdate } from "./events/messageUpdate";
import { onReady } from "./events/ready";
import { CommandManager } from "./utils/commandManager";
import { closeAllMysqlPools } from "./utils/mysql-pool";
import { telegramCrosspostService } from "./utils/telegram-crosspost/service";

const gatewayIntents = [
  GatewayIntentBits.Guilds,
  GatewayIntentBits.GuildMembers,
  GatewayIntentBits.GuildMessages,
  GatewayIntentBits.GuildMessageReactions,
];
if (process.env.TELEGRAM_CROSSPOST_ROUTES?.trim()) {
  gatewayIntents.push(GatewayIntentBits.MessageContent);
}

const client = new Client({
  allowedMentions: {
    // Prevent @everyone pings
    parse: ["users", "roles"],
  },
  intents: gatewayIntents,
  partials: [Partials.Message, Partials.Channel, Partials.Reaction],
});

const commandManager = new CommandManager();

async function runEventHandlers(label: string, handlers: Promise<unknown>[]) {
  const results = await Promise.allSettled(handlers);
  for (const result of results) {
    if (result.status === "rejected") {
      console.error(`[${label}] Handler failed:`, result.reason);
    }
  }
}

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

client.once("clientReady", async (readyClient) => {
  await runEventHandlers("Ready", [
    onReady(readyClient),
    telegramCrosspostService.onReady(readyClient),
  ]);
});
client.on("messageCreate", async (message: Message) => {
  await runEventHandlers("MessageCreate", [
    onMessageCreate(client, message),
    telegramCrosspostService.onMessageCreate(message),
  ]);
});
client.on(
  "messageUpdate",
  async (
    oldMessage: Message | PartialMessage,
    newMessage: Message | PartialMessage,
  ) => {
    await runEventHandlers("MessageUpdate", [
      onMessageUpdate(client, oldMessage, newMessage),
      telegramCrosspostService.onMessageUpdate(newMessage),
    ]);
  },
);
client.on(
  "interactionCreate",
  async (interaction: Interaction) =>
    await onInteractionCreate(commandManager, interaction),
);
client.on("messageDelete", async (message: Message | PartialMessage) => {
  await telegramCrosspostService.onMessageDelete(message);
});
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
    console.log("Waiting for Telegram crosspost jobs...");
    await telegramCrosspostService.shutdown();

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
