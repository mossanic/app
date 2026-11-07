package org.mossanic.app

import android.app.Activity
import android.os.Bundle
import android.util.Log
import android.view.Surface
import android.view.SurfaceHolder
import android.view.SurfaceView

class MainActivity : Activity() {
    private lateinit var surfaceView: SurfaceView

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        
        System.loadLibrary("mossanic_renderer")
        System.loadLibrary("mossanic_gles")
        
        surfaceView = SurfaceView(this)
        setContentView(surfaceView)

        surfaceView.holder.addCallback(object : SurfaceHolder.Callback {
            override fun surfaceCreated(holder: SurfaceHolder) {
                // Try Vulkan first. If it fails, fall back to GLES.
                val vulkanSuccess = NativeBridge.setSurfaceVulkan(holder.surface)
                if (!vulkanSuccess) {
                    Log.w("Mossanic", "Vulkan failed. Falling back to GLES.")
                    NativeBridge.setSurfaceGLES(holder.surface)
                }
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
    external fun setSurfaceVulkan(surface: Surface): Boolean
    external fun setSurfaceGLES(surface: Surface): Boolean
    external fun resize(width: Int, height: Int)
    external fun clearSurface()
}
