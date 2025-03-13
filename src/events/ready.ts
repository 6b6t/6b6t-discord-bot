import { Client } from 'discord.js';
import {sync} from "./sync";

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

    void runSync();
};
