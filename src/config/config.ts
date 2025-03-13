
const config = {
    clientId: '1328715386371309619',
    guildId: '1326869396324614245',
    redis: {
        host: 'redis',
        port: 6379,
        channels: {
            rankUpdates: 'rank_updates'
        }
    },
    mysql: {
        host: 'mariadb',
        port: 3306,
        user: 'root',
        password: 'devenv',
        database: 'linked_players'
    },
};

export default config;
