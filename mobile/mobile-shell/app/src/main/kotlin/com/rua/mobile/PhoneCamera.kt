package com.rua.mobile

import android.Manifest
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Matrix
import android.hardware.camera2.CameraCharacteristics
import android.hardware.camera2.CameraManager
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.widget.Button
import android.widget.LinearLayout
import android.widget.TextView
import androidx.activity.ComponentActivity
import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageCapture
import androidx.camera.core.ImageCaptureException
import androidx.camera.core.ImageProxy
import androidx.camera.core.Preview
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.view.PreviewView
import androidx.core.content.ContextCompat
import org.json.JSONArray
import org.json.JSONObject
import java.io.ByteArrayOutputStream
import java.util.UUID
import java.util.concurrent.CompletableFuture

internal data class PhonePhotoRequest(val token: String, val cameraId: String, val operation: PhoneOperation)

internal object PhoneCamera {
    @Volatile private var pending: PhonePhotoRequest? = null
    @Volatile private var activity: PhoneCameraActivity? = null
    fun list(context: Context): JSONObject {
        val manager=context.getSystemService(Context.CAMERA_SERVICE) as CameraManager
        val cameras=JSONArray()
        for(id in manager.cameraIdList) {
            val facing=manager.getCameraCharacteristics(id).get(CameraCharacteristics.LENS_FACING)
            if(facing==CameraCharacteristics.LENS_FACING_FRONT || facing==CameraCharacteristics.LENS_FACING_BACK)
                cameras.put(JSONObject().put("camera_id",id).put("facing",if(facing==CameraCharacteristics.LENS_FACING_FRONT) "front" else "back"))
        }
        return JSONObject().put("cameras",cameras)
    }
    @Synchronized fun take(service: Context, owner: String, id: String, cameraId: String): CompletableFuture<JSONObject> {
        check(ContextCompat.checkSelfPermission(service,Manifest.permission.CAMERA)==PackageManager.PERMISSION_GRANTED) { "Grant camera permission in Rua before taking a photo" }
        val cameras=list(service).getJSONArray("cameras")
        check((0 until cameras.length()).any { cameras.getJSONObject(it).getString("camera_id")==cameraId }) { "Unknown camera" }
        val op=PhoneDeviceControl.state.begin(owner,id)
        val request=PhonePhotoRequest(UUID.randomUUID().toString(),cameraId,op)
        pending=request
        val handler=Handler(Looper.getMainLooper())
        handler.post {
            if(op.cancelled.get()) {finish(request,IllegalStateException("Camera cancelled"));return@post}
            try {service.startActivity(Intent(service,PhoneCameraActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK).putExtra("request",request.token))}
            catch(e:Exception){finish(request,e)}
        }
        handler.postDelayed({ if(!op.result.isDone) finish(request,IllegalStateException("Open Rua camera preview; capture timed out")) },12_000)
        return op.result
    }
    fun claim(token:String?):PhonePhotoRequest?=pending?.takeIf {it.token==token && !it.operation.result.isDone}
    fun attach(value:PhoneCameraActivity) { activity=value }
    fun detach(value:PhoneCameraActivity) { if(activity === value) activity=null }
    @Synchronized fun finish(request:PhonePhotoRequest,error:Throwable?=null,image:JSONObject?=null){
        if(pending !== request) return
        if(Looper.myLooper()!=Looper.getMainLooper()) {
            Handler(Looper.getMainLooper()).post {finish(request,error,image)}
            return
        }
        activity?.releaseCamera()
        if(error!=null) request.operation.result.completeExceptionally(error)
        else request.operation.result.complete(JSONObject().put("image",image).put("camera_id",request.cameraId))
        PhoneDeviceControl.state.finish(request.operation)
        pending=null
    }
}

class PhoneCameraActivity : ComponentActivity() {
    private var request:PhonePhotoRequest?=null
    private var provider:ProcessCameraProvider?=null
    private val handler=Handler(Looper.getMainLooper())
    private var captured=false
    override fun onCreate(savedInstanceState:Bundle?){
        super.onCreate(savedInstanceState)
        request=PhoneCamera.claim(intent.getStringExtra("request"))
        val r=request ?: run {finish();return}
        PhoneCamera.attach(this)
        val column=LinearLayout(this).apply {orientation=LinearLayout.VERTICAL}
        column.addView(TextView(this).apply {text="Rua 单次拍照 · 相机 ${r.cameraId}";textSize=20f;setPadding(20,30,20,20)})
        val previewView=PreviewView(this)
        column.addView(previewView,LinearLayout.LayoutParams(-1,0,1f))
        if (MobileAccessibilityService.instance == null) {
            column.addView(Button(this).apply {text="终止";contentDescription="终止当前拍照";setOnClickListener {PhoneDeviceControl.state.cancel(r.operation.owner,r.operation.requestId);PhoneCamera.finish(r,IllegalStateException("Photo cancelled by user"));finish()}})
        }
        setContentView(column)
        val future=ProcessCameraProvider.getInstance(this)
        future.addListener({
            if(isFinishing || r.operation.result.isDone || r.operation.cancelled.get())return@addListener
            try {
                provider=future.get()
                val selector=CameraSelector.Builder().addCameraFilter { infos -> infos.filter { androidx.camera.camera2.interop.Camera2CameraInfo.from(it).cameraId==r.cameraId } }.build()
                val preview=Preview.Builder().build().also {it.setSurfaceProvider(previewView.surfaceProvider)}
                val capture=ImageCapture.Builder().setCaptureMode(ImageCapture.CAPTURE_MODE_MINIMIZE_LATENCY).build()
                provider!!.bindToLifecycle(this,selector,preview,capture)
                previewView.previewStreamState.observe(this) { stream ->
                    if(stream==PreviewView.StreamState.STREAMING && !captured){captured=true;handler.postDelayed({capture(r,capture)},350)}
                }
            }catch(e:Exception){PhoneCamera.finish(r,e);finish()}
        },ContextCompat.getMainExecutor(this))
        handler.post(object:Runnable{override fun run(){
            if(isFinishing)return
            if(r.operation.result.isDone || r.operation.cancelled.get() || ContextCompat.checkSelfPermission(this@PhoneCameraActivity,Manifest.permission.CAMERA)!=PackageManager.PERMISSION_GRANTED) {PhoneCamera.finish(r,IllegalStateException("Camera cancelled or permission revoked"));finish();return}
            handler.postDelayed(this,50)
        }})
    }

    private fun capture(r:PhonePhotoRequest,capture:ImageCapture){
        if(isFinishing || r.operation.result.isDone || r.operation.cancelled.get())return
        try { PhoneDeviceControl.state.dispatch(r.operation) { capture.takePicture(ContextCompat.getMainExecutor(this),object:ImageCapture.OnImageCapturedCallback(){
            override fun onCaptureSuccess(image:ImageProxy){
                var bitmap:Bitmap?=null
                try {
                    check(!r.operation.result.isDone && !r.operation.cancelled.get()) {"Camera cancelled"}
                    val buffer=image.planes[0].buffer
                    val bytes=ByteArray(buffer.remaining());buffer.get(bytes)
                    bitmap=BitmapFactory.decodeByteArray(bytes,0,bytes.size) ?: error("Camera image decode failed")
                    val rotation=image.imageInfo.rotationDegrees
                    if(rotation!=0){val rotated=Bitmap.createBitmap(bitmap,0,0,bitmap.width,bitmap.height,Matrix().apply {postRotate(rotation.toFloat())},true);bitmap.recycle();bitmap=rotated}
                    val factor=minOf(1.0,1280.0/maxOf(bitmap.width,bitmap.height))
                    val scaled=Bitmap.createScaledBitmap(bitmap,(bitmap.width*factor).toInt().coerceAtLeast(1),(bitmap.height*factor).toInt().coerceAtLeast(1),true)
                    if(scaled!==bitmap){bitmap.recycle();bitmap=scaled}
                    val output=ByteArrayOutputStream();check(bitmap.compress(Bitmap.CompressFormat.JPEG,70,output)) {"Photo encoding failed"}
                    check(output.size()<=768*1024){"Photo exceeds size limit"}
                    PhoneCamera.finish(r,image=JSONObject().put("mime_type","image/jpeg").put("width",bitmap.width).put("height",bitmap.height).put("data",android.util.Base64.encodeToString(output.toByteArray(),android.util.Base64.NO_WRAP)))
                }catch(e:Exception){PhoneCamera.finish(r,e)}finally{bitmap?.recycle();image.close();finish()}
            }
            override fun onError(exception:ImageCaptureException){PhoneCamera.finish(r,exception);finish()}
        }) } } catch(e:Exception){PhoneCamera.finish(r,e);finish()}
    }
    override fun onStop(){
        request?.let { if(!it.operation.result.isDone)PhoneCamera.finish(it,IllegalStateException("Camera preview is no longer visible")) }
        provider?.unbindAll()
        super.onStop()
    }
    internal fun releaseCamera(){provider?.unbindAll();provider=null}
    override fun onDestroy(){handler.removeCallbacksAndMessages(null);releaseCamera();request?.let {PhoneCamera.finish(it,IllegalStateException("Camera closed"))};PhoneCamera.detach(this);super.onDestroy()}
}
