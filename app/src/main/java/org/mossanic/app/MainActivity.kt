package org.mossanic.app

import android.app.Activity
import android.os.Bundle
import android.view.Surface
import android.view.SurfaceHolder
import android.view.SurfaceView

class MainActivity : Activity() {
    private lateinit var surfaceView: SurfaceView

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        
        System.loadLibrary("mossanic_renderer")
        
        surfaceView = SurfaceView(this)
        setContentView(surfaceView)

        surfaceView.holder.addCallback(object : SurfaceHolder.Callback {
            override fun surfaceCreated(holder: SurfaceHolder) {
                NativeBridge.setSurface(holder.surface)
            }
            override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) {
                NativeBridge.resize(width, height)
            }
            override fun surfaceDestroyed(holder: SurfaceHolder) {
                NativeBridge.clearSurface()
            }
        })
    }
}

object NativeBridge {
    external fun setSurface(surface: Surface)
    external fun resize(width: Int, height: Int)
    external fun clearSurface()
}
