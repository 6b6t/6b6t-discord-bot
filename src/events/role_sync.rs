use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

use futures::{StreamExt as _, TryStreamExt as _, stream};
use poise::serenity_prelude as serenity;

use crate::{
    config,
    server::UserInfo,
    state::{AppState, CachedUserInfo},
};

type RoleSpec = (&'static str, serenity::RoleId, fn(&UserInfo) -> bool);

const ROLE_SPECS: &[RoleSpec] = &[
    (
        "prime",
        serenity::RoleId::new(1_268_337_190_144_835_718),
        |info| info.top_rank == "prime",
    ),
    (
        "primeultra",
        serenity::RoleId::new(1_325_147_393_372_586_054),
        |info| info.top_rank == "primeultra",
    ),
    (
        "elite",
        serenity::RoleId::new(1_268_337_279_898_878_013),
        |info| info.top_rank == "elite",
    ),
    (
        "eliteultra",
        serenity::RoleId::new(1_325_147_417_322_192_927),
        |info| info.top_rank == "eliteultra",
    ),
    (
        "apex",
        serenity::RoleId::new(1_268_345_919_003_430_942),
        |info| info.top_rank == "apex",
    ),
    (
        "legend",
        serenity::RoleId::new(1_349_026_308_390_391_839),
        |info| info.top_rank == "legend",
    ),
    (
        "2022",
        serenity::RoleId::new(1_349_065_372_313_321_514),
        |info| info.first_join_year <= 2022,
    ),
    (
        "2023",
        serenity::RoleId::new(1_349_065_403_477_004_480),
        |info| info.first_join_year == 2023,
    ),
    (
        "2024",
        serenity::RoleId::new(1_349_065_422_065_893_516),
        |info| info.first_join_year == 2024,
    ),
    (
        "2025",
        serenity::RoleId::new(1_349_065_443_650_043_955),
        |info| info.first_join_year == 2025,
    ),
    (
        "2026",
        serenity::RoleId::new(1_453_085_388_930_416_702),
        |info| info.first_join_year == 2026,
    ),
];

pub async fn run(ctx: &serenity::Context, data: &AppState) {
    if let Err(error) = run_inner(ctx, data).await {
        tracing::error!(%error, "linked role synchronization failed");
    }
}

async fn run_inner(ctx: &serenity::Context, data: &AppState) -> anyhow::Result<()> {
    let Some(databases) = &data.databases else {
        return Ok(());
    };
    let mappings = databases.mappings().await?;
    let members = config::GUILD_ID
        .members_iter(&ctx.http)
        .try_collect::<Vec<_>>()
        .await?;
    let member_map = members
        .into_iter()
        .map(|member| (member.user.id, member))
        .collect::<HashMap<_, _>>();
    let linked_ids = mappings
        .iter()
        .filter_map(|mapping| mapping.discord_id.parse::<u64>().ok())
        .map(serenity::UserId::new)
        .collect::<HashSet<_>>();
    let bypass = member_map
        .values()
        .filter(|member| member.roles.contains(&config::MANUALLY_MANAGED_ROLE_ID))
        .map(|member| member.user.id)
        .collect::<HashSet<_>>();
    sync_role(
        ctx,
        "linked",
        config::LINKED_ROLE_ID,
        &linked_ids,
        &member_map,
        &bypass,
        |_| true,
    )
    .await;

    let results = stream::iter(mappings).map(|mapping| async move {
        let user_id = mapping.discord_id.parse::<u64>().ok().map(serenity::UserId::new)?;
        let cached = data.role_sync_cache.read().await.get(&mapping.uuid).cloned();
        if let Some(cached) = cached.filter(|value| value.expires_at > std::time::Instant::now()) { return Some((user_id, cached.value)); }
        match data.server.user_info(&mapping.uuid).await {
            Ok(Some(info)) => {
                data.role_sync_cache.write().await.insert(mapping.uuid, CachedUserInfo { value: info.clone(), expires_at: std::time::Instant::now() + Duration::from_mins(5) });
                Some((user_id, info))
            }
            Ok(None) => None,
            Err(error) => { tracing::warn!(%error, user_id = %user_id, "failed to resolve linked user information"); None }
        }
    }).buffer_unordered(8).filter_map(|value| async move { value }).collect::<HashMap<_, _>>().await;
    let resolved_ids = results.keys().copied().collect::<HashSet<_>>();
    for (name, role_id, predicate) in ROLE_SPECS {
        let allowed = results
            .iter()
            .filter(|(_, info)| predicate(info))
            .map(|(id, _)| *id)
            .collect::<HashSet<_>>();
        sync_role(ctx, name, *role_id, &allowed, &member_map, &bypass, |id| {
            !linked_ids.contains(&id) || resolved_ids.contains(&id)
        })
        .await;
    }
    Ok(())
}

async fn sync_role(
    ctx: &serenity::Context,
    name: &str,
    role_id: serenity::RoleId,
    allowed: &HashSet<serenity::UserId>,
    members: &HashMap<serenity::UserId, serenity::Member>,
    bypass: &HashSet<serenity::UserId>,
    removable: impl Fn(serenity::UserId) -> bool,
) {
    let current = members
        .values()
        .filter(|member| member.roles.contains(&role_id))
        .map(|member| member.user.id)
        .collect::<HashSet<_>>();
    let additions = allowed
        .difference(&current)
        .filter(|id| !bypass.contains(id))
        .copied()
        .collect::<Vec<_>>();
    let removals = current
        .difference(allowed)
        .filter(|id| !bypass.contains(id) && removable(**id))
        .copied()
        .collect::<Vec<_>>();
    for id in &additions {
        if let Some(member) = members.get(id)
            && let Err(error) = member.add_role(ctx, role_id).await
        {
            tracing::error!(%error, role = name, user_id = %id, "failed to add synchronized role");
        }
    }
    for id in &removals {
        if let Some(member) = members.get(id)
            && let Err(error) = member.remove_role(ctx, role_id).await
        {
            tracing::error!(%error, role = name, user_id = %id, "failed to remove synchronized role");
        }
    }
    tracing::info!(
        role = name,
        add = additions.len(),
        remove = removals.len(),
        "synchronized linked role"
    );
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    #[test]
    fn bypassed_members_are_not_changed() {
        let allowed = HashSet::from([1, 2]);
        let current = HashSet::from([2, 3]);
        let bypass = HashSet::from([1, 3]);
        let add = allowed
            .difference(&current)
            .filter(|id| !bypass.contains(id))
            .copied()
            .collect::<Vec<_>>();
        let remove = current
            .difference(&allowed)
            .filter(|id| !bypass.contains(id))
            .copied()
            .collect::<Vec<_>>();
        assert!(add.is_empty());
        assert!(remove.is_empty());
    }
}
