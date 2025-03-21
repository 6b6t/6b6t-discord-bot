import { Client, TextChannel } from 'discord.js';
import {sync} from "./sync";
import cron from 'cron';
import config from "../config/config";

export const onReady = (client: Client) => {
    console.log(`Logged in as ${client.user?.tag}!`);
    async function runSync() {
        console.log("Running sync...");
        try {
            if (client.isReady()) {
                await sync(client);
            }
        } finally {
            setTimeout(runSync, 30_000);
        }
    }

    async function sendReminder() {
        const channel = client.channels.cache.get(config.generalId) as TextChannel;
        if (channel) {
          await channel.send(config.generalMessage);
        } else {
          console.error(`Couldn't find general channel by ID: ${config.generalId}`)
        }
    };

    new cron.CronJob(
        "0 10 * * *",
        sendReminder,
        null,
        true,
        "Europe/Berlin"
    );
    
    new cron.CronJob(
        "0 18 * * *",
        sendReminder,
        null,
        true,
        "Europe/Berlin"
    );

    void runSync();
};
