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

    async storeLinkCode(code: string, playerUuid: string): Promise<boolean> {
        try {
            // Store code with 5 minute expiration
            await this.pool.execute(
                'INSERT INTO link_codes (code, player_uuid, expires_at) VALUES (?, ?, DATE_ADD(NOW(), INTERVAL 5 MINUTE)) ' +
                'ON DUPLICATE KEY UPDATE code = VALUES(code), expires_at = VALUES(expires_at)',
                [code, playerUuid]
            );
            return true;
        } catch (error) {
            console.error('Error storing link code:', error);
            return false;
        }
    }

    async verifyLinkCode(code: string, discordId: string): Promise<boolean> {
        try {
            // Get and validate unexpired code
            const [rows] = await this.pool.execute<mysql.RowDataPacket[]>(
                'SELECT player_uuid FROM link_codes WHERE code = ? AND expires_at > NOW()',
                [code]
            );

            if (rows.length === 0) {
                return false;
            }

            const playerUuid = rows[0].player_uuid;
            
            // Delete the used code
            await this.pool.execute('DELETE FROM link_codes WHERE code = ?', [code]);
            
            // Link the player
            return await this.linkPlayer(playerUuid, discordId);
        } catch (error) {
            console.error('Error verifying link code:', error);
            return false;
        }
    }

    async close() {
        await this.pool.end();
    }
}
