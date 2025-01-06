import { Client } from 'discord.js';

export const onReady = (client: Client) => {
    console.log(`Logged in as ${client.user?.tag}!`);
};