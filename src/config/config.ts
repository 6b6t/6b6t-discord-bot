export type RankRoleKey = 'group.prime' | 'group.elite' | 'group.apex' | 'group.primeultra' | 'group.eliteultra';

const config = {
    token: '',
    clientId: '1325506526571532462',
    guildId: '917520262797344779',
    allowedUsers: ['1192360689411575828'],
    redis: {
        host: 'localhost',
        port: 6379,
        channels: {
            rankUpdates: 'rank_updates'
        }
    },
    mysql: {
        host: 'localhost',
        port: 3306,
        user: 'linking',
        password: '1234',
        database: 'linking'
    },
    rankRoles: {
        'group.prime': '1268337190144835718',
        'group.elite': '1268337279898878013',
        'group.apex': '1268345919003430942',
        'group.primeultra': '1325147393372586054',
        'group.eliteultra': '1325147417322192927'
    } as Record<RankRoleKey, string>,
    linkedRole: '1325507259307921428'
};

export default config;