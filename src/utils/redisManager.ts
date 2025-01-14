import { createClient } from 'redis';
import config, { RankRoleKey } from '../config/config';
import { Client, GuildMember } from 'discord.js';
import { DatabaseManager } from './databaseManager';

interface RankUpdate {
    playerUuid: string;
    rank: string;
    action: string;
}

export class RedisManager {
    private subscriber!: ReturnType<typeof createClient>;
    private client!: ReturnType<typeof createClient>;
    private discordClient: Client;
    private dbManager: DatabaseManager;

    constructor(discordClient: Client) {
        this.discordClient = discordClient;
        this.dbManager = new DatabaseManager();
        this.initializeRedis();
    }

    private async initializeRedis() {
        try {
            console.log('Attempting to connect to Redis host:', config.redis.host);
            
            this.subscriber = createClient({
                socket: {
                    host: config.redis.host,
                    port: Number(config.redis.port),
                }
            });

            this.client = createClient({
                socket: {
                    host: config.redis.host,
                    port: Number(config.redis.port),
                }
            });

            await this.subscriber.connect();
            await this.client.connect();

            await this.subscriber.subscribe(config.redis.channels.rankUpdates, (message) => {
                this.handleRankUpdate(message);
            });

            console.log('Redis connections established');
        } catch (error) {
            console.error('Redis connection error:', error);
        }
    }

    private async getGuild() {
        const guild = await this.discordClient.guilds.fetch(config.guildId);
        if (!guild) {
            throw new Error('Bot is not in the configured guild');
        }
        return guild;
    }

    public async verifyLinkCode(code: string, discordId: string): Promise<boolean> {
        try {
            const playerUuid = await this.client.get(`code:${code}`);
            if (!playerUuid) {
                console.log(`No player UUID found for code: ${code}`);
                return false;
            }

            const success = await this.dbManager.linkPlayer(playerUuid, discordId);
            if (success) {
                try {
                    const guild = await this.getGuild();
                    const member = await guild.members.fetch(discordId);
                    if (member) {
                        await member.roles.add(config.linkedRole);
                    }
                } catch (error) {
                    console.error('Error adding linked role:', error);
                    return true;
                }

                await this.client.del(`code:${code}`);
                return true;
            }
        } catch (error) {
            console.error('Error verifying link code:', error);
        }

        return false;
    }

    private async handleRankUpdate(message: string) {
        try {
            const regex = /Player (.*) has (.*) rank (.*)/;
            const matches = message.match(regex);

            if (!matches) {
                console.error('Invalid rank update message format:', message);
                return;
            }

            const [, playerUuid, action, rank] = matches;

            const update: RankUpdate = {
                playerUuid,
                action,
                rank
            };

            await this.updateMemberRoles(update);
        } catch (error) {
            console.error('Error handling rank update:', error);
        }
    }

    private async updateMemberRoles(update: RankUpdate) {
        try {
            const discordId = await this.dbManager.getDiscordId(update.playerUuid);
            if (!discordId) {
                console.log(`No Discord ID found for player UUID: ${update.playerUuid}`);
                return;
            }

            const roleId = config.rankRoles[update.rank.toLowerCase() as RankRoleKey];
            if (!roleId) {
                console.log(`No role ID configured for rank: ${update.rank}`);
                return;
            }

            const guild = await this.getGuild();
            const member = await guild.members.fetch(discordId);
            if (!member) {
                console.log(`Member ${discordId} not found in guild`);
                return;
            }

            if (update.action === 'achieved') {
                if (!member.roles.cache.has(roleId)) {
                    await member.roles.add(roleId);
                    console.log(`Added role ${update.rank} to member ${discordId}`);
                }
            } else if (update.action === 'lost') {
                if (member.roles.cache.has(roleId)) {
                    await member.roles.remove(roleId);
                    console.log(`Removed role ${update.rank} from member ${discordId}`);
                }
            }
        } catch (error) {
            console.error('Error updating member roles:', error);
        }
    }

    public async close() {
        try {
            await this.subscriber.disconnect();
            await this.client.disconnect();
            await this.dbManager.close();
            console.log('Redis and Database connections closed');
        } catch (error) {
            console.error('Error closing connections:', error);
            throw error;
        }
    }
} 
