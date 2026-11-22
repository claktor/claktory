package com.claktory.app

import android.app.Application

/** The process's first code: sets the tunnel up before any Dart runs. */
class ClaktoryApplication : Application() {
    override fun onCreate() {
        super.onCreate()
        TunnelBootstrap.install(this)
    }
}
