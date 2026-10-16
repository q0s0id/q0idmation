package com.q0id.q0player

import com.q0id.android.Q0Activity

class MainActivity : Q0Activity() {
    companion object {
        init {
            System.loadLibrary("q0player_android")
        }
    }
}
