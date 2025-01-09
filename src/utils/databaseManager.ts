import mysql from 'mysql2/promise';
import config from '../config/config';

export class DatabaseManager {
    private pool: mysql.Pool;

    constructor() {
        this.pool = mysql.createPool({
            host: config.mysql.host,
            port: Number(config.mysql.port),
            user: config.mysql.user,
            password: config.mysql.password,
            database: config.mysql.database,
            waitForConnections: true,
            connectionLimit: 10,
            queueLimit: 0
        });
    }

    async getDiscordId(playerUuid: string): Promise<string | null> {
        try {
            const [rows] = await this.pool.execute<mysql.RowDataPacket[]>(
                'SELECT discord_id FROM linked_players WHERE player_uuid = ?',
                [playerUuid]
            );

            if (rows.length > 0) {
                return rows[0].discord_id;
            }
            return null;
        } catch (error) {
            console.error('Error fetching Discord ID:', error);
            return null;
        }
    }

    async linkPlayer(playerUuid: string, discordId: string): Promise<boolean> {
        try {
            await this.pool.execute(
                'INSERT INTO linked_players (player_uuid, discord_id) VALUES (?, ?)',
                [playerUuid, discordId]
            );
            return true;
        } catch (error) {
            console.error('Error linking player:', error);
            return false;
        }
    }

    async close() {
        await this.pool.end();
    }
} 
