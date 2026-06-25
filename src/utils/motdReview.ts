import {
  ActionRowBuilder,
  type ButtonInteraction,
  type GuildMember,
  MessageFlags,
  ModalBuilder,
  type ModalSubmitInteraction,
  TextInputBuilder,
  TextInputStyle,
} from "discord.js";

type MotdReviewAction = "approve" | "revision";

type MotdReviewResponse = {
  success: boolean;
  error?: string;
  data?: {
    requestId: string;
    status: string;
    message: string;
  };
};

function getMotdReviewApiUrl() {
  const explicitUrl = process.env.MOTD_REVIEW_API_URL;
  if (explicitUrl) return explicitUrl;

  const baseUrl = process.env.WEBSITE_BASE_URL ?? "https://www.6b6t.org";
  return `${baseUrl.replace(/\/$/, "")}/api/discord/motd/review`;
}

function parseMotdCustomId(customId: string): {
  action: "approve" | "revision" | "revision_modal";
  requestId: string;
} | null {
  const [prefix, action, ...requestIdParts] = customId.split(":");
  const requestId = requestIdParts.join(":");

  if (
    prefix !== "motd" ||
    !requestId ||
    (action !== "approve" &&
      action !== "revision" &&
      action !== "revision_modal")
  ) {
    return null;
  }

  return { action, requestId };
}

function getMemberRoleIds(member: GuildMember): string[] {
  return member.roles.cache.map((role) => role.id);
}

function getReviewerUsername(member: GuildMember) {
  return member.user.globalName ?? member.user.tag ?? member.user.username;
}

async function postMotdReviewAction(input: {
  action: MotdReviewAction;
  requestId: string;
  member: GuildMember;
  reason?: string;
}): Promise<MotdReviewResponse> {
  const secret = process.env.MOTD_REVIEW_BOT_SECRET;
  if (!secret) {
    return {
      success: false,
      error: "MOTD_REVIEW_BOT_SECRET is not configured on the bot.",
    };
  }

  const response = await fetch(getMotdReviewApiUrl(), {
    method: "POST",
    headers: {
      Authorization: `Bearer ${secret}`,
      "Content-Type": "application/json",
    },
    body: JSON.stringify({
      action: input.action,
      requestId: input.requestId,
      reason: input.reason,
      reviewer: {
        id: input.member.id,
        username: getReviewerUsername(input.member),
        roleIds: getMemberRoleIds(input.member),
      },
    }),
  });

  const payload = (await response
    .json()
    .catch(() => null)) as MotdReviewResponse | null;

  if (!response.ok) {
    return {
      success: false,
      error:
        payload?.error ??
        `Website returned ${response.status}${
          response.statusText ? ` ${response.statusText}` : ""
        }`,
    };
  }

  return (
    payload ?? { success: false, error: "Website returned an empty body." }
  );
}

function createRevisionModal(requestId: string) {
  const reasonInput = new TextInputBuilder()
    .setCustomId("reason")
    .setLabel("Reason")
    .setStyle(TextInputStyle.Paragraph)
    .setMinLength(3)
    .setMaxLength(1000)
    .setPlaceholder("Explain what needs to be changed.")
    .setRequired(true);

  return new ModalBuilder()
    .setCustomId(`motd:revision_modal:${requestId}`)
    .setTitle("Request MOTD Revision")
    .addComponents(
      new ActionRowBuilder<TextInputBuilder>().addComponents(reasonInput),
    );
}

export function isMotdReviewInteraction(customId: string): boolean {
  return customId.startsWith("motd:");
}

export async function handleMotdReviewButton(interaction: ButtonInteraction) {
  const parsed = parseMotdCustomId(interaction.customId);
  if (!parsed) return false;

  if (parsed.action === "revision") {
    await interaction.showModal(createRevisionModal(parsed.requestId));
    return true;
  }

  if (parsed.action !== "approve") {
    await interaction.reply({
      content: "Unknown MOTD action.",
      flags: MessageFlags.Ephemeral,
    });
    return true;
  }

  const member = interaction.member as GuildMember | null;
  if (!member) {
    await interaction.reply({
      content: "Could not identify your server membership.",
      flags: MessageFlags.Ephemeral,
    });
    return true;
  }

  await interaction.deferReply({ flags: MessageFlags.Ephemeral });

  const result = await postMotdReviewAction({
    action: "approve",
    requestId: parsed.requestId,
    member,
  });

  await interaction.editReply({
    content: result.success
      ? (result.data?.message ?? `Approved MOTD request ${parsed.requestId}.`)
      : `Failed to approve MOTD: ${result.error ?? "Unknown error"}`,
  });

  return true;
}

export async function handleMotdReviewModal(
  interaction: ModalSubmitInteraction,
) {
  const parsed = parseMotdCustomId(interaction.customId);
  if (parsed?.action !== "revision_modal") return false;

  const member = interaction.member as GuildMember | null;
  if (!member) {
    await interaction.reply({
      content: "Could not identify your server membership.",
      flags: MessageFlags.Ephemeral,
    });
    return true;
  }

  const reason = interaction.fields.getTextInputValue("reason").trim();
  await interaction.deferReply({ flags: MessageFlags.Ephemeral });

  const result = await postMotdReviewAction({
    action: "revision",
    requestId: parsed.requestId,
    member,
    reason,
  });

  await interaction.editReply({
    content: result.success
      ? (result.data?.message ??
        `Requested revision for MOTD request ${parsed.requestId}.`)
      : `Failed to request revision: ${result.error ?? "Unknown error"}`,
  });

  return true;
}
