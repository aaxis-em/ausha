package com.ausha.receiver

import android.net.Uri

/**
 * A pairing link, as carried by the QR code the sender prints:
 * `ausha://<host>:<port>?token=<token>&name=<sender>`
 */
data class Pairing(val host: String, val port: Int, val token: String, val name: String?) {

    companion object {
        const val SCHEME = "ausha"

        fun parse(text: String): Pairing? {
            val uri = runCatching { Uri.parse(text.trim()) }.getOrNull() ?: return null
            if (!uri.scheme.equals(SCHEME, ignoreCase = true)) return null
            val host = uri.host ?: return null
            val token = uri.getQueryParameter("token") ?: return null
            if (host.isBlank() || token.isBlank()) return null
            return Pairing(
                host = host,
                port = uri.port.takeIf { it > 0 } ?: DEFAULT_PORT,
                token = token,
                name = uri.getQueryParameter("name"),
            )
        }

        const val DEFAULT_PORT = 6996
    }
}
