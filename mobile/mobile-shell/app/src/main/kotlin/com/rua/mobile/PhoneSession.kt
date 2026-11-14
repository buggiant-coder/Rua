package com.rua.mobile

import org.json.JSONObject
import java.net.URLEncoder
import java.util.UUID

private fun identity(value: String, name: String, limit: Int = 200): String {
    require(value.isNotBlank() && value.length <= limit) { "Invalid Phone Use $name" }
    return value
}

internal data class PhoneChatAddress(val kind: String, val resourceId: String, val chatId: String) {
    init {
        require(kind == "agent" || kind == "project") { "Invalid Phone Use Chat scope" }
        identity(resourceId, "resource ID")
        identity(chatId, "Chat ID")
    }

    fun apiPath(): String {
        fun segment(value: String): String = URLEncoder.encode(value, "UTF-8").replace("+", "%20")
            .let { if (it == "." || it == "..") it.replace(".", "%2E") else it }
        val scope = if (kind == "agent") "agents" else "projects"
        return "/$scope/${segment(resourceId)}/chats/${segment(chatId)}"
    }

    companion object {
        fun fromJson(value: JSONObject): PhoneChatAddress {
            val scope = value.getJSONObject("scope")
            val kind = scope.getString("kind")
            val resourceKey = if (kind == "agent") "agent_id" else "project_id"
            return PhoneChatAddress(kind, scope.getString(resourceKey), value.getString("chat_id"))
        }
    }
}

internal data class PhoneInvocation(
    val owner: String,
    val chatAddress: PhoneChatAddress,
    val turnId: String,
    val authorityEpoch: String,
    val configurationEpoch: String,
) {
    init {
        identity(owner, "owner", 128)
        identity(turnId, "Turn ID")
        identity(authorityEpoch, "authority epoch")
        identity(configurationEpoch, "configuration epoch")
    }

    fun toJson(): JSONObject = JSONObject().put("owner", owner)
        .put("chat_address", JSONObject()
            .put("scope", JSONObject().put("kind", chatAddress.kind)
                .put(if (chatAddress.kind == "agent") "agent_id" else "project_id", chatAddress.resourceId))
            .put("chat_id", chatAddress.chatId))
        .put("turn_id", turnId)
        .put("authority_epoch", authorityEpoch)
        .put("chat_configuration_epoch", configurationEpoch)

    companion object {
        fun fromParams(params: JSONObject): PhoneInvocation {
            val value = params.getJSONObject("invocation")
            return PhoneInvocation(
                value.getString("owner"), PhoneChatAddress.fromJson(value.getJSONObject("chat_address")),
                value.getString("turn_id"), value.getString("authority_epoch"),
                value.getString("chat_configuration_epoch"),
            )
        }
    }
}

internal data class PhoneUseLease(val sessionId: String, val invocation: PhoneInvocation)

/** One visible session; completed operations do not release its lease. */
internal class PhoneSessionState {
    private val lock = Any()
    private var active: PhoneUseLease? = null
    private val retired = HashSet<Pair<String, String>>()
    private val revoked = HashSet<Pair<String, String>>()
    @Volatile var onChanged: (() -> Unit)? = null

    fun current(): PhoneUseLease? = synchronized(lock) { active }

    fun activate(inv: PhoneInvocation): PhoneUseLease = change {
        checkAvailable(inv)
        val previous = active
        if (previous == null) {
            PhoneUseLease(UUID.randomUUID().toString(), inv).also { active = it }
        } else {
            check(sameChat(previous.invocation, inv)) { "Phone Use busy with another Chat" }
            adopt(previous, inv)
        }
    }

    fun bindTurn(inv: PhoneInvocation): Boolean = change {
        checkAvailable(inv)
        val previous = active
        if (previous == null || !sameChat(previous.invocation, inv)) false else {
            adopt(previous, inv)
            true
        }
    }

    fun isCurrent(inv: PhoneInvocation): Boolean = synchronized(lock) { active?.invocation == inv }

    fun release(inv: PhoneInvocation): Boolean = change {
        if (active?.invocation != inv) false else {
            active = null
            true
        }
    }

    fun stop(expectedSessionId: String): PhoneInvocation? = change {
        active?.takeIf { it.sessionId == expectedSessionId }?.invocation?.also {
            revoked.add(it.owner to it.turnId)
            active = null
        }
    }

    fun clear() = change { active = null }

    fun reset() = change {
        active = null
        retired.clear()
        revoked.clear()
    }

    private fun checkAvailable(inv: PhoneInvocation) {
        check((inv.owner to inv.turnId) !in revoked) { "Phone Use session was stopped for this Turn" }
        check((inv.owner to inv.turnId) !in retired) { "Phone Use Turn was superseded" }
    }

    private fun sameChat(first: PhoneInvocation, second: PhoneInvocation): Boolean =
        first.owner == second.owner && first.chatAddress == second.chatAddress

    private fun adopt(previous: PhoneUseLease, inv: PhoneInvocation): PhoneUseLease {
        if (previous.invocation.turnId == inv.turnId) {
            check(previous.invocation == inv) { "Phone Use invocation changed within the same Turn" }
            return previous
        }
        retired.add(previous.invocation.owner to previous.invocation.turnId)
        return previous.copy(invocation = inv).also { active = it }
    }

    private fun <T> change(action: () -> T): T {
        val (result, notify) = synchronized(lock) {
            val before = active
            val result = action()
            result to if (before != active) onChanged else null
        }
        notify?.invoke()
        return result
    }
}
