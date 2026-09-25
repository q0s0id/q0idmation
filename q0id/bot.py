from __future__ import annotations

import asyncio
import logging
import os
import sqlite3
import time
from dataclasses import dataclass
from pathlib import Path

import discord
from discord import app_commands
from discord.ext import commands


BASE_DIR = Path(__file__).resolve().parent
ENV_PATH = BASE_DIR / ".env"
DB_PATH = BASE_DIR / "q0id_bot.sqlite3"

XP_COOLDOWN_SECONDS = 60
LEVELS = (
    # level, role name, minimum xp, fallback role color
    (1, "I0", 0, 0x8E9297),
    (2, "Q0", 250, 0x57F287),
    (3, "Q0ID", 800, 0x5865F2),
    (4, "Q0S", 1800, 0xEB459E),
    (5, "Q0S0ID", 3500, 0xFEE75C),
)

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s | %(levelname)s | %(name)s | %(message)s",
)
log = logging.getLogger("q0id-bot")


@dataclass(frozen=True)
class EnvConfig:
    token: str
    test_guild_id: int | None = None


def load_env() -> EnvConfig:
    """Accept both KEY=VALUE .env files and the current raw-value format."""
    named: dict[str, str] = {}
    raw: list[str] = []

    if ENV_PATH.exists():
        for original_line in ENV_PATH.read_text(encoding="utf-8-sig").splitlines():
            line = original_line.strip()
            if not line or line.startswith("#"):
                continue
            if "=" in line:
                key, value = line.split("=", 1)
                named[key.strip().upper()] = value.strip().strip('"').strip("'")
            else:
                raw.append(line)

    token = os.getenv("DISCORD_TOKEN") or named.get("DISCORD_TOKEN")
    if not token and raw:
        token = raw[0]

    if not token:
        raise RuntimeError(
            "discord token not found. put DISCORD_TOKEN=... in .env "
            "or keep the token as the first non-empty line."
        )

    guild_raw = os.getenv("TEST_GUILD_ID") or named.get("TEST_GUILD_ID")
    test_guild_id = int(guild_raw) if guild_raw and guild_raw.isdigit() else None
    return EnvConfig(token=token, test_guild_id=test_guild_id)


class Storage:
    def __init__(self, path: Path) -> None:
        self.conn = sqlite3.connect(path, check_same_thread=False)
        self.conn.row_factory = sqlite3.Row
        self.lock = asyncio.Lock()
        self._create_schema()

    def _create_schema(self) -> None:
        self.conn.executescript(
            """
            PRAGMA journal_mode=WAL;
            PRAGMA foreign_keys=ON;

            CREATE TABLE IF NOT EXISTS member_progress (
                guild_id INTEGER NOT NULL,
                user_id INTEGER NOT NULL,
                xp INTEGER NOT NULL DEFAULT 0,
                last_xp_at REAL NOT NULL DEFAULT 0,
                PRIMARY KEY (guild_id, user_id)
            );

            CREATE TABLE IF NOT EXISTS warnings (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                guild_id INTEGER NOT NULL,
                user_id INTEGER NOT NULL,
                moderator_id INTEGER NOT NULL,
                reason TEXT NOT NULL,
                created_at REAL NOT NULL,
                revoked_at REAL,
                revoked_by INTEGER
            );

            CREATE INDEX IF NOT EXISTS idx_warnings_active
            ON warnings (guild_id, user_id, revoked_at);
            """
        )
        self.conn.commit()

    async def get_progress(self, guild_id: int, user_id: int) -> tuple[int, float]:
        async with self.lock:
            row = self.conn.execute(
                "SELECT xp, last_xp_at FROM member_progress WHERE guild_id=? AND user_id=?",
                (guild_id, user_id),
            ).fetchone()
            if row is None:
                self.conn.execute(
                    "INSERT INTO member_progress (guild_id, user_id, xp, last_xp_at) VALUES (?, ?, 0, 0)",
                    (guild_id, user_id),
                )
                self.conn.commit()
                return 0, 0.0
            return int(row["xp"]), float(row["last_xp_at"])

    async def grant_message_xp(self, guild_id: int, user_id: int, now: float) -> tuple[int, int, bool]:
        async with self.lock:
            row = self.conn.execute(
                "SELECT xp, last_xp_at FROM member_progress WHERE guild_id=? AND user_id=?",
                (guild_id, user_id),
            ).fetchone()

            if row is None:
                xp = 0
                last_xp_at = 0.0
            else:
                xp = int(row["xp"])
                last_xp_at = float(row["last_xp_at"])

            if now - last_xp_at < XP_COOLDOWN_SECONDS:
                return xp, xp, False

            new_xp = xp + 1
            self.conn.execute(
                """
                INSERT INTO member_progress (guild_id, user_id, xp, last_xp_at)
                VALUES (?, ?, ?, ?)
                ON CONFLICT(guild_id, user_id)
                DO UPDATE SET xp=excluded.xp, last_xp_at=excluded.last_xp_at
                """,
                (guild_id, user_id, new_xp, now),
            )
            self.conn.commit()
            return xp, new_xp, True

    async def set_xp(self, guild_id: int, user_id: int, xp: int) -> None:
        async with self.lock:
            self.conn.execute(
                """
                INSERT INTO member_progress (guild_id, user_id, xp, last_xp_at)
                VALUES (?, ?, ?, 0)
                ON CONFLICT(guild_id, user_id)
                DO UPDATE SET xp=excluded.xp
                """,
                (guild_id, user_id, xp),
            )
            self.conn.commit()

    async def active_warning_count(self, guild_id: int, user_id: int) -> int:
        async with self.lock:
            row = self.conn.execute(
                "SELECT COUNT(*) AS n FROM warnings WHERE guild_id=? AND user_id=? AND revoked_at IS NULL",
                (guild_id, user_id),
            ).fetchone()
            return int(row["n"])

    async def add_warning(self, guild_id: int, user_id: int, moderator_id: int, reason: str) -> tuple[int, int]:
        async with self.lock:
            cur = self.conn.execute(
                """
                INSERT INTO warnings (guild_id, user_id, moderator_id, reason, created_at)
                VALUES (?, ?, ?, ?, ?)
                """,
                (guild_id, user_id, moderator_id, reason, time.time()),
            )
            warning_id = int(cur.lastrowid)
            row = self.conn.execute(
                "SELECT COUNT(*) AS n FROM warnings WHERE guild_id=? AND user_id=? AND revoked_at IS NULL",
                (guild_id, user_id),
            ).fetchone()
            self.conn.commit()
            return warning_id, int(row["n"])

    async def list_active_warnings(self, guild_id: int, user_id: int) -> list[sqlite3.Row]:
        async with self.lock:
            return list(
                self.conn.execute(
                    """
                    SELECT id, moderator_id, reason, created_at
                    FROM warnings
                    WHERE guild_id=? AND user_id=? AND revoked_at IS NULL
                    ORDER BY id ASC
                    """,
                    (guild_id, user_id),
                ).fetchall()
            )

    async def revoke_latest_warning(self, guild_id: int, user_id: int, moderator_id: int) -> int | None:
        async with self.lock:
            row = self.conn.execute(
                """
                SELECT id FROM warnings
                WHERE guild_id=? AND user_id=? AND revoked_at IS NULL
                ORDER BY id DESC LIMIT 1
                """,
                (guild_id, user_id),
            ).fetchone()
            if row is None:
                return None
            warning_id = int(row["id"])
            self.conn.execute(
                "UPDATE warnings SET revoked_at=?, revoked_by=? WHERE id=?",
                (time.time(), moderator_id, warning_id),
            )
            self.conn.commit()
            return warning_id


storage = Storage(DB_PATH)


def level_for_xp(xp: int) -> int:
    level = 1
    for candidate, _role_name, minimum_xp, _color in LEVELS:
        if xp >= minimum_xp:
            level = candidate
    return level


def level_data(level: int) -> tuple[int, str, int, int]:
    for data in LEVELS:
        if data[0] == level:
            return data
    raise ValueError(f"invalid level: {level}")


def next_level_data(level: int) -> tuple[int, str, int, int] | None:
    return next((data for data in LEVELS if data[0] == level + 1), None)


def role_by_name(guild: discord.Guild, name: str) -> discord.Role | None:
    target = name.casefold()
    matching = [role for role in guild.roles if role.name.casefold() == target]
    return max(matching, key=lambda role: role.position) if matching else None


async def ensure_level_roles(guild: discord.Guild) -> dict[int, discord.Role]:
    result: dict[int, discord.Role] = {}
    me = guild.me
    if me is None:
        raise RuntimeError("bot member is not available in this guild")

    for level, role_name, _minimum_xp, fallback_color in LEVELS:
        role = role_by_name(guild, role_name)
        if role is None:
            if not me.guild_permissions.manage_roles:
                raise RuntimeError(f"missing role {role_name} and bot has no Manage Roles permission")
            role = await guild.create_role(
                name=role_name,
                colour=discord.Colour(fallback_color),
                reason="q0id level role setup",
            )
            log.info("created level role %s in guild %s", role_name, guild.id)
        result[level] = role
    return result


class RoleHierarchyError(RuntimeError):
    pass


async def sync_level_role(member: discord.Member, level: int, *, reason: str) -> None:
    roles = await ensure_level_roles(member.guild)
    target = roles[level]
    me = member.guild.me
    if me is None:
        raise RoleHierarchyError("bot member is unavailable")

    level_role_ids = {role.id for role in roles.values()}
    current_level_roles = [role for role in member.roles if role.id in level_role_ids]

    roles_to_remove = [role for role in current_level_roles if role.id != target.id]
    affected = roles_to_remove + ([] if target in member.roles else [target])
    blocked = [role for role in affected if role >= me.top_role]
    if blocked:
        names = ", ".join(role.name for role in blocked)
        raise RoleHierarchyError(
            f"my discord role must be above these level roles: {names}"
        )

    if roles_to_remove:
        await member.remove_roles(*roles_to_remove, reason=reason)
    if target not in member.roles:
        await member.add_roles(target, reason=reason)


def human_level(level: int) -> str:
    return level_data(level)[1]


def can_moderate(actor: discord.Member, target: discord.Member) -> tuple[bool, str | None]:
    guild = actor.guild
    if target.id == actor.id:
        return False, "you cannot use this moderation action on yourself."
    if target.id == guild.owner_id:
        return False, "the server owner cannot be moderated by the bot."
    if actor.id != guild.owner_id and target.top_role >= actor.top_role:
        return False, "that member has a role equal to or above your highest role."
    return True, None


def bot_can_ban(guild: discord.Guild, target: discord.Member) -> tuple[bool, str | None]:
    me = guild.me
    if me is None:
        return False, "bot member is unavailable."
    if not me.guild_permissions.ban_members:
        return False, "the bot needs the Ban Members permission."
    if target.id == guild.owner_id or target.top_role >= me.top_role:
        return False, "the bot role must be above the target member's highest role."
    return True, None


class Q0IDBot(commands.Bot):
    def __init__(self, config: EnvConfig) -> None:
        intents = discord.Intents.default()
        intents.members = True
        intents.messages = True
        super().__init__(command_prefix=commands.when_mentioned, intents=intents)
        self.config = config

    async def setup_hook(self) -> None:
        if self.config.test_guild_id:
            guild = discord.Object(id=self.config.test_guild_id)
            self.tree.copy_global_to(guild=guild)
            synced = await self.tree.sync(guild=guild)
            log.info("synced %d commands to test guild %s", len(synced), guild.id)
        else:
            synced = await self.tree.sync()
            log.info("synced %d global commands", len(synced))


config = load_env()
bot = Q0IDBot(config)


@bot.event
async def on_ready() -> None:
    assert bot.user is not None
    log.info("logged in as %s (%s)", bot.user, bot.user.id)
    for guild in bot.guilds:
        try:
            await ensure_level_roles(guild)
        except Exception:
            log.exception("could not ensure level roles in guild %s", guild.id)


@bot.event
async def on_guild_join(guild: discord.Guild) -> None:
    try:
        await ensure_level_roles(guild)
    except Exception:
        log.exception("could not set up guild %s", guild.id)


@bot.event
async def on_member_join(member: discord.Member) -> None:
    if member.bot:
        return
    try:
        await sync_level_role(member, 1, reason="q0id starting level")
    except Exception:
        log.exception("could not assign starting role to %s", member.id)


@bot.event
async def on_message(message: discord.Message) -> None:
    if message.guild is None or message.author.bot:
        return
    if not isinstance(message.author, discord.Member):
        return

    old_xp, new_xp, granted = await storage.grant_message_xp(
        message.guild.id,
        message.author.id,
        time.time(),
    )
    if not granted:
        return

    old_level = level_for_xp(old_xp)
    new_level = level_for_xp(new_xp)

    try:
        await sync_level_role(
            message.author,
            new_level,
            reason=f"q0id xp level sync ({new_xp} xp)",
        )
    except Exception:
        log.exception("failed to sync level role for member %s", message.author.id)
        return

    if new_level > old_level:
        try:
            await message.channel.send(
                f"{message.author.mention} достиг уровня **{new_level} · {human_level(new_level)}** ✦"
            )
        except discord.HTTPException:
            pass


@bot.tree.command(name="level", description="show q0id level and xp")
@app_commands.describe(member="member to inspect; leave empty for yourself")
async def level_command(interaction: discord.Interaction, member: discord.Member | None = None) -> None:
    if interaction.guild is None:
        await interaction.response.send_message("this command only works in a server.", ephemeral=True)
        return

    target = member or interaction.user
    if not isinstance(target, discord.Member):
        await interaction.response.send_message("member not found.", ephemeral=True)
        return

    xp, _ = await storage.get_progress(interaction.guild.id, target.id)
    level = level_for_xp(xp)
    nxt = next_level_data(level)

    if nxt is None:
        progress = "maximum level reached"
    else:
        needed = nxt[2] - xp
        progress = f"{needed} xp until {nxt[1]}"

    await interaction.response.send_message(
        f"**{target.display_name}** · level **{level} / 5** · **{human_level(level)}**\n"
        f"xp: **{xp}** · {progress}"
    )


@bot.tree.command(name="setlevel", description="set a member's q0id level")
@app_commands.describe(member="member", level="level from 1 to 5")
@app_commands.checks.has_permissions(manage_roles=True)
async def setlevel_command(
    interaction: discord.Interaction,
    member: discord.Member,
    level: app_commands.Range[int, 1, 5],
) -> None:
    if interaction.guild is None:
        await interaction.response.send_message("this command only works in a server.", ephemeral=True)
        return

    minimum_xp = level_data(level)[2]
    await storage.set_xp(interaction.guild.id, member.id, minimum_xp)
    try:
        await sync_level_role(
            member,
            level,
            reason=f"manual level set by {interaction.user} ({interaction.user.id})",
        )
    except RoleHierarchyError as exc:
        await interaction.response.send_message(str(exc), ephemeral=True)
        return

    await interaction.response.send_message(
        f"{member.mention} теперь уровень **{level} · {human_level(level)}**."
    )


@bot.tree.command(name="warn", description="warn a member; the third active warning permanently bans them")
@app_commands.describe(member="member to warn", reason="reason for the warning")
@app_commands.checks.has_permissions(moderate_members=True)
async def warn_command(
    interaction: discord.Interaction,
    member: discord.Member,
    reason: app_commands.Range[str, 1, 400],
) -> None:
    if interaction.guild is None or not isinstance(interaction.user, discord.Member):
        await interaction.response.send_message("this command only works in a server.", ephemeral=True)
        return

    allowed, why = can_moderate(interaction.user, member)
    if not allowed:
        await interaction.response.send_message(why or "cannot moderate that member.", ephemeral=True)
        return

    current_count = await storage.active_warning_count(interaction.guild.id, member.id)
    if current_count >= 3:
        await interaction.response.send_message(
            "this member already has 3 active warnings.", ephemeral=True
        )
        return

    will_ban = current_count == 2
    if will_ban:
        allowed, why = bot_can_ban(interaction.guild, member)
        if not allowed:
            await interaction.response.send_message(
                f"this would be warning 3/3, but i cannot enforce the permanent ban: {why}",
                ephemeral=True,
            )
            return

    warning_id, count = await storage.add_warning(
        interaction.guild.id,
        member.id,
        interaction.user.id,
        reason,
    )

    if count >= 3:
        try:
            await member.ban(
                reason=f"q0id: third warning #{warning_id}; {reason}"
            )
        except discord.HTTPException as exc:
            log.exception("third-warning ban failed for %s", member.id)
            await interaction.response.send_message(
                f"warning **3/3** was recorded, but discord rejected the ban: `{exc}`",
                ephemeral=True,
            )
            return

        await interaction.response.send_message(
            f"⛔ {member} получил предупреждение **3/3** и забанен навсегда.\n"
            f"причина: **{reason}**"
        )
        return

    await interaction.response.send_message(
        f"⚠️ {member.mention} получил предупреждение **{count}/3**.\n"
        f"причина: **{reason}**"
    )


@bot.tree.command(name="warns", description="show a member's active warnings")
@app_commands.describe(member="member to inspect")
@app_commands.checks.has_permissions(moderate_members=True)
async def warns_command(interaction: discord.Interaction, member: discord.Member) -> None:
    if interaction.guild is None:
        await interaction.response.send_message("this command only works in a server.", ephemeral=True)
        return

    warnings = await storage.list_active_warnings(interaction.guild.id, member.id)
    if not warnings:
        await interaction.response.send_message(
            f"{member.display_name} has no active warnings.", ephemeral=True
        )
        return

    lines: list[str] = []
    for row in warnings:
        timestamp = int(float(row["created_at"]))
        lines.append(
            f"**#{row['id']}** · <t:{timestamp}:d> · <@{row['moderator_id']}>\n"
            f"{row['reason']}"
        )

    await interaction.response.send_message(
        f"warnings for **{member.display_name}**: **{len(warnings)}/3**\n\n" + "\n\n".join(lines),
        ephemeral=True,
    )


@bot.tree.command(name="unwarn", description="remove a member's latest active warning")
@app_commands.describe(member="member")
@app_commands.checks.has_permissions(moderate_members=True)
async def unwarn_command(interaction: discord.Interaction, member: discord.Member) -> None:
    if interaction.guild is None:
        await interaction.response.send_message("this command only works in a server.", ephemeral=True)
        return

    warning_id = await storage.revoke_latest_warning(
        interaction.guild.id,
        member.id,
        interaction.user.id,
    )
    if warning_id is None:
        await interaction.response.send_message(
            f"{member.display_name} has no active warnings.", ephemeral=True
        )
        return

    count = await storage.active_warning_count(interaction.guild.id, member.id)
    await interaction.response.send_message(
        f"removed warning **#{warning_id}** from {member.mention}. now **{count}/3** active warnings."
    )


@bot.tree.command(name="ban", description="permanently ban a member")
@app_commands.describe(member="member to ban", reason="ban reason")
@app_commands.checks.has_permissions(ban_members=True)
async def ban_command(
    interaction: discord.Interaction,
    member: discord.Member,
    reason: app_commands.Range[str, 1, 400],
) -> None:
    if interaction.guild is None or not isinstance(interaction.user, discord.Member):
        await interaction.response.send_message("this command only works in a server.", ephemeral=True)
        return

    allowed, why = can_moderate(interaction.user, member)
    if not allowed:
        await interaction.response.send_message(why or "cannot ban that member.", ephemeral=True)
        return

    allowed, why = bot_can_ban(interaction.guild, member)
    if not allowed:
        await interaction.response.send_message(why or "the bot cannot ban that member.", ephemeral=True)
        return

    await member.ban(reason=f"q0id manual ban by {interaction.user}: {reason}")
    await interaction.response.send_message(
        f"⛔ **{member}** забанен навсегда.\nпричина: **{reason}**"
    )


@bot.tree.error
async def on_app_command_error(interaction: discord.Interaction, error: app_commands.AppCommandError) -> None:
    if isinstance(error, app_commands.MissingPermissions):
        message = "у тебя нет нужных прав для этой команды."
    elif isinstance(error, app_commands.BotMissingPermissions):
        missing = ", ".join(error.missing_permissions)
        message = f"боту не хватает прав: {missing}."
    else:
        log.exception("app command error", exc_info=error)
        message = "команда упала с ошибкой. подробности напечатаны в консоли бота."

    if interaction.response.is_done():
        await interaction.followup.send(message, ephemeral=True)
    else:
        await interaction.response.send_message(message, ephemeral=True)


if __name__ == "__main__":
    bot.run(config.token, log_handler=None)
