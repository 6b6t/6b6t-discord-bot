export type RankRoleKey = 'group.prime' | 'group.elite' | 'group.apex' | 'group.primeultra' | 'group.eliteultra';

const config = {
    token: 'MTMyODcxNTM4NjM3MTMwOTYxOQ.GwoZsC.T0zyps43eOsiJ9J8Z2fXNP9Q8IMxbmUaGsHd1U',
    clientId: '1328715386371309619',
    guildId: '1326869396324614245',
    allowedUsers: ['367842772025081856'],
    redis: {
        host: 'redis',
        port: 6379,
        channels: {
            rankUpdates: 'rank_updates'
        }
    },
    mysql: {
        host: 'localhost',
        port: 3306,
        user: 'root',
        password: 'devenv',
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
