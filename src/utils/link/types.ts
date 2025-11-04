export type DiscordTokens = {
  access_token: string;
  refresh_token: string;
  expires_in: number;
  expires_at: number;
};

export type DiscordRoleMetadata = Record<string, string | number>;
