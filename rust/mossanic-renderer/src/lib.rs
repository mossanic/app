use ash::vk;
use ash::Entry;
use jni::objects::{JClass, JObject};
use jni::JNIEnv;
use m2s2_math::Matrix4x4f32;
use mossanic_pty::Pty;
use mossanic_terminal_core::Terminal;
use once_cell::sync::Lazy;
use std::ffi::{CStr, CString};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread;

use android_raw_window::AndroidNativeWindow;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

mod font;
mod surface_transform;
mod text_grid;
use font::FONT_DATA;
use surface_transform::{
    pre_rotation as surface_pre_rotation, swaps_extent as surface_transform_swaps_extent,
};
use text_grid::{build_grid_vertices_into, Vertex, ATLAS_HEIGHT, ATLAS_WIDTH};

static VULKAN_INIT: Lazy<Mutex<bool>> = Lazy::new(|| Mutex::new(false));
static SHOULD_CLOSE: AtomicBool = AtomicBool::new(false);
static VULKAN_THREAD: Lazy<Mutex<Option<thread::JoinHandle<()>>>> = Lazy::new(|| Mutex::new(None));

static VERT_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/text.vert.spv"));
static FRAG_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/text.frag.spv"));

// --- Screen Buffer ---
struct ScreenBuffer {
    cols: usize,
    rows: usize,
    cells: Vec<u8>,
    cursor_x: usize,
    cursor_y: usize,
}

impl ScreenBuffer {
    fn new(cols: usize, rows: usize) -> Self {
        Self {
            cols,
            rows,
            cells: vec![b' '; cols * rows],
            cursor_x: 0,
            cursor_y: 0,
        }
    }

    fn push_char(&mut self, c: u8) {
        if c == b'\n' {
            self.cursor_y += 1;
            self.cursor_x = 0;
        } else if c == b'\r' {
            self.cursor_x = 0;
        } else if (32..=126).contains(&c) && self.cursor_y < self.rows && self.cursor_x < self.cols
        {
            self.cells[self.cursor_y * self.cols + self.cursor_x] = c;
            self.cursor_x += 1;
        }

        // Wrap
        if self.cursor_x >= self.cols {
            self.cursor_x = 0;
            self.cursor_y += 1;
        }

        // Scroll
        if self.cursor_y >= self.rows {
            self.cells.drain(0..self.cols);
            self.cells.extend_from_slice(&vec![b' '; self.cols]);
            self.cursor_y = self.rows - 1;
        }
    }
}

static SCREEN: Lazy<Mutex<ScreenBuffer>> = Lazy::new(|| Mutex::new(ScreenBuffer::new(80, 24)));
static TERMINAL: Lazy<Mutex<Terminal>> = Lazy::new(|| Mutex::new(Terminal::new(80, 24)));

#[no_mangle]
pub extern "system" fn Java_org_mossanic_NativeBridge_setSurfaceVulkan(
    env: JNIEnv,
    _class: JClass,
    surface: JObject,
) -> jni::sys::jboolean {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Trace)
            .with_tag("MossanicRust"),
    );
    log::info!("JNI: setSurfaceVulkan called");

    if SHOULD_CLOSE.load(Ordering::SeqCst) {
        if let Some(previous_thread) = VULKAN_THREAD.lock().unwrap().take() {
            if previous_thread.join().is_err() {
                log::error!("Previous Vulkan render thread panicked during shutdown");
            }
        }
    }
    let mut init_guard = VULKAN_INIT.lock().unwrap();
    if *init_guard {
        return 1;
    }
    let native_window = unsafe {
        ndk::native_window::NativeWindow::from_surface(
            env.get_native_interface() as *mut _,
            surface.as_raw() as *mut _,
        )
    };
    let Some(native_window) = native_window else {
        log::error!("Failed to create ANativeWindow");
        return 0;
    };
    let window_ptr = native_window.ptr().as_ptr() as usize;
    // Transfer the acquired ANativeWindow reference into the render thread.
    std::mem::forget(native_window);
    *init_guard = true;
    SHOULD_CLOSE.store(false, Ordering::SeqCst);
    let render_thread = thread::spawn(move || {
        let _window_owner = unsafe {
            ndk::native_window::NativeWindow::from_ptr(
                std::ptr::NonNull::new(window_ptr as *mut _)
                    .expect("Android returned a null native window"),
            )
        };
        let result = std::panic::catch_unwind(|| unsafe {
            run_vulkan(window_ptr);
        });
        if result.is_err() {
            log::error!("VULKAN THREAD PANICKED!");
        }
        *VULKAN_INIT.lock().unwrap() = false;
    });
    *VULKAN_THREAD.lock().unwrap() = Some(render_thread);
    1
}

#[no_mangle]
pub extern "system" fn Java_org_mossanic_NativeBridge_startShell(_env: JNIEnv, _class: JClass) {
    log::info!("JNI: startShell called");
    std::thread::spawn(|| {
        let pty = match Pty::spawn("/system/bin/sh", &[], 24, 80) {
            Ok(p) => p,
            Err(e) => {
                log::error!("Failed to spawn PTY: {}", e);
                return;
            }
        };

        log::info!("PTY spawned successfully.");
        std::thread::sleep(std::time::Duration::from_millis(100));
        let _ = pty.write(b"echo Hello from mossanic-pty!\n");

        let mut buf = [0u8; 1024];
        loop {
            match pty.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let mut screen = SCREEN.lock().unwrap();
                    for &b in &buf[..n] {
                        screen.push_char(b);
                    }
                    TERMINAL.lock().unwrap().feed(&buf[..n]);
                }
                Err(e) => {
                    log::error!("PTY read error: {}", e);
                    break;
                }
            }
        }
        log::info!("PTY loop ended.");
    });
}

#[no_mangle]
pub extern "system" fn Java_org_mossanic_NativeBridge_clearSurface(_env: JNIEnv, _class: JClass) {
    SHOULD_CLOSE.store(true, Ordering::SeqCst);
    *VULKAN_INIT.lock().unwrap() = false;
}

#[no_mangle]
pub extern "system" fn Java_org_mossanic_NativeBridge_resize(
    _env: JNIEnv,
    _class: JClass,
    _w: i32,
    _h: i32,
) {
}

unsafe fn run_vulkan(window_ptr: usize) {
    log::info!("Starting Vulkan initialization...");
    let entry = Entry::load().expect("Failed to load Vulkan");

    let native_window = AndroidNativeWindow(window_ptr as *mut _);
    let window_handle = native_window.window_handle().unwrap();
    let display_handle = native_window.display_handle().unwrap();
    let raw_display_handle = display_handle.as_raw();
    let raw_window_handle = window_handle.as_raw();

    let app_name = CString::new("MossanicSpike").unwrap();
    let app_info = vk::ApplicationInfo {
        p_application_name: app_name.as_ptr(),
        api_version: vk::make_api_version(0, 1, 1, 0),
        ..Default::default()
    };

    let instance_exts = ash_window::enumerate_required_extensions(raw_display_handle).unwrap();
    let instance_create_info = vk::InstanceCreateInfo {
        p_application_info: &app_info,
        enabled_extension_count: instance_exts.len() as u32,
        pp_enabled_extension_names: instance_exts.as_ptr(),
        ..Default::default()
    };
    let instance = entry.create_instance(&instance_create_info, None).unwrap();

    let surface = ash_window::create_surface(
        &entry,
        &instance,
        raw_display_handle,
        raw_window_handle,
        None,
    )
    .expect("Failed to create Vulkan surface");
    let surface_ext = ash::khr::surface::Instance::new(&entry, &instance);

    let (physical_device, queue_family_index) = instance
        .enumerate_physical_devices()
        .unwrap()
        .into_iter()
        .find_map(|physical_device| {
            let has_swapchain_extension = instance
                .enumerate_device_extension_properties(physical_device)
                .map(|extensions| {
                    extensions.iter().any(|extension| unsafe {
                        CStr::from_ptr(extension.extension_name.as_ptr())
                            == ash::khr::swapchain::NAME
                    })
                })
                .unwrap_or(false);
            if !has_swapchain_extension {
                return None;
            }
            instance
                .get_physical_device_queue_family_properties(physical_device)
                .iter()
                .enumerate()
                .find_map(|(index, properties)| {
                    let can_present = surface_ext
                        .get_physical_device_surface_support(physical_device, index as u32, surface)
                        .unwrap_or(false);
                    (properties.queue_flags.contains(vk::QueueFlags::GRAPHICS) && can_present)
                        .then_some((physical_device, index as u32))
                })
        })
        .expect("No Vulkan queue family supports both graphics and Android presentation");
    let device_exts = [ash::khr::swapchain::NAME.as_ptr()];
    let queue_priorities = [1.0];
    let queue_create_info = vk::DeviceQueueCreateInfo {
        queue_family_index,
        queue_count: 1,
        p_queue_priorities: queue_priorities.as_ptr(),
        ..Default::default()
    };
    let device_create_info = vk::DeviceCreateInfo {
        queue_create_info_count: 1,
        p_queue_create_infos: &queue_create_info,
        enabled_extension_count: device_exts.len() as u32,
        pp_enabled_extension_names: device_exts.as_ptr(),
        ..Default::default()
    };
    let device = instance
        .create_device(physical_device, &device_create_info, None)
        .unwrap();
    let queue = device.get_device_queue(queue_family_index, 0);
    let _mem_props = instance.get_physical_device_memory_properties(physical_device);

    let caps = surface_ext
        .get_physical_device_surface_capabilities(physical_device, surface)
        .unwrap();
    let formats = surface_ext
        .get_physical_device_surface_formats(physical_device, surface)
        .unwrap();
    let format = *formats.first().expect("Surface has no supported formats");
    let present_modes = surface_ext
        .get_physical_device_surface_present_modes(physical_device, surface)
        .unwrap();
    let present_mode = if present_modes.contains(&vk::PresentModeKHR::MAILBOX) {
        vk::PresentModeKHR::MAILBOX
    } else {
        vk::PresentModeKHR::FIFO
    };
    if !caps
        .supported_usage_flags
        .contains(vk::ImageUsageFlags::COLOR_ATTACHMENT)
    {
        panic!("Surface images cannot be used as color attachments");
    }

    let surface_extent = if caps.current_extent.width != u32::MAX {
        caps.current_extent
    } else {
        vk::Extent2D {
            width: (native_window.get_width().max(1) as u32)
                .clamp(caps.min_image_extent.width, caps.max_image_extent.width),
            height: (native_window.get_height().max(1) as u32)
                .clamp(caps.min_image_extent.height, caps.max_image_extent.height),
        }
    };
    let (width, height) = if surface_transform_swaps_extent(caps.current_transform) {
        (surface_extent.height, surface_extent.width)
    } else {
        (surface_extent.width, surface_extent.height)
    };
    log::info!(
        "Surface transform=0x{:x}; pre-rotated swapchain extent={}x{}",
        caps.current_transform.as_raw(),
        width,
        height
    );
    let image_count = if caps.max_image_count > 0 {
        caps.min_image_count
            .saturating_add(1)
            .min(caps.max_image_count)
    } else {
        caps.min_image_count.saturating_add(1)
    };
    let composite_alpha = [
        vk::CompositeAlphaFlagsKHR::OPAQUE,
        vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED,
        vk::CompositeAlphaFlagsKHR::POST_MULTIPLIED,
        vk::CompositeAlphaFlagsKHR::INHERIT,
    ]
    .into_iter()
    .find(|alpha| caps.supported_composite_alpha.contains(*alpha))
    .expect("Surface has no supported composite-alpha mode");

    let swapchain_create_info = vk::SwapchainCreateInfoKHR {
        surface,
        min_image_count: image_count,
        image_format: format.format,
        image_color_space: format.color_space,
        image_extent: vk::Extent2D { width, height },
        image_array_layers: 1,
        image_usage: vk::ImageUsageFlags::COLOR_ATTACHMENT,
        image_sharing_mode: vk::SharingMode::EXCLUSIVE,
        // Android pre-rotation: the projection below applies current_transform.
        pre_transform: caps.current_transform,
        composite_alpha,
        present_mode,
        clipped: vk::TRUE,
        ..Default::default()
    };
    let swapchain_loader = ash::khr::swapchain::Device::new(&instance, &device);
    let swapchain = swapchain_loader
        .create_swapchain(&swapchain_create_info, None)
        .unwrap();
    let images = swapchain_loader.get_swapchain_images(swapchain).unwrap();

    let attachments = [vk::AttachmentDescription {
        format: format.format,
        samples: vk::SampleCountFlags::TYPE_1,
        load_op: vk::AttachmentLoadOp::CLEAR,
        store_op: vk::AttachmentStoreOp::STORE,
        stencil_load_op: vk::AttachmentLoadOp::DONT_CARE,
        stencil_store_op: vk::AttachmentStoreOp::DONT_CARE,
        initial_layout: vk::ImageLayout::UNDEFINED,
        final_layout: vk::ImageLayout::PRESENT_SRC_KHR,
        ..Default::default()
    }];
    let color_refs = [vk::AttachmentReference {
        attachment: 0,
        layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
    }];
    let subpasses = [vk::SubpassDescription {
        pipeline_bind_point: vk::PipelineBindPoint::GRAPHICS,
        color_attachment_count: 1,
        p_color_attachments: color_refs.as_ptr(),
        ..Default::default()
    }];
    let dependencies = [vk::SubpassDependency {
        src_subpass: vk::SUBPASS_EXTERNAL,
        dst_subpass: 0,
        src_stage_mask: vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
        dst_stage_mask: vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
        src_access_mask: vk::AccessFlags::empty(),
        dst_access_mask: vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
        dependency_flags: vk::DependencyFlags::empty(),
    }];
    let render_pass = device
        .create_render_pass(
            &vk::RenderPassCreateInfo {
                attachment_count: 1,
                p_attachments: attachments.as_ptr(),
                subpass_count: 1,
                p_subpasses: subpasses.as_ptr(),
                dependency_count: 1,
                p_dependencies: dependencies.as_ptr(),
                ..Default::default()
            },
            None,
        )
        .unwrap();

    let mut framebuffers = Vec::with_capacity(images.len());
    let mut swapchain_image_views = Vec::with_capacity(images.len());
    for image in &images {
        let view = device
            .create_image_view(
                &vk::ImageViewCreateInfo {
                    image: *image,
                    view_type: vk::ImageViewType::TYPE_2D,
                    format: format.format,
                    subresource_range: vk::ImageSubresourceRange {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        level_count: 1,
                        layer_count: 1,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        framebuffers.push(
            device
                .create_framebuffer(
                    &vk::FramebufferCreateInfo {
                        render_pass,
                        attachment_count: 1,
                        p_attachments: &view,
                        width,
                        height,
                        layers: 1,
                        ..Default::default()
                    },
                    None,
                )
                .unwrap(),
        );
        swapchain_image_views.push(view);
    }

    // --- Build a single-row R8 bitmap font atlas (95 printable ASCII glyphs) ---
    let mut atlas_data = vec![0u8; (ATLAS_WIDTH * ATLAS_HEIGHT) as usize];
    for (glyph_index, glyph_rows) in FONT_DATA.iter().enumerate() {
        for (y, row_bits) in glyph_rows.iter().enumerate() {
            for x in 0..8 {
                let bit = (row_bits >> (7 - x)) & 1;
                let index = y * ATLAS_WIDTH as usize + glyph_index * 8 + x;
                atlas_data[index] = if bit == 1 { 255 } else { 0 };
            }
        }
    }

    // --- Upload Texture ---
    let image_info = vk::ImageCreateInfo {
        image_type: vk::ImageType::TYPE_2D,
        format: vk::Format::R8_UNORM,
        extent: vk::Extent3D {
            width: ATLAS_WIDTH,
            height: ATLAS_HEIGHT,
            depth: 1,
        },
        mip_levels: 1,
        array_layers: 1,
        samples: vk::SampleCountFlags::TYPE_1,
        tiling: vk::ImageTiling::OPTIMAL,
        usage: vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST,
        initial_layout: vk::ImageLayout::UNDEFINED,
        ..Default::default()
    };
    let texture_image = device.create_image(&image_info, None).unwrap();
    let mem_reqs = device.get_image_memory_requirements(texture_image);
    let mem_type_index = (0.._mem_props.memory_type_count)
        .find(|&i| {
            (mem_reqs.memory_type_bits & (1 << i)) != 0
                && _mem_props.memory_types[i as usize]
                    .property_flags
                    .contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)
        })
        .unwrap();
    let texture_memory = device
        .allocate_memory(
            &vk::MemoryAllocateInfo {
                allocation_size: mem_reqs.size,
                memory_type_index: mem_type_index,
                ..Default::default()
            },
            None,
        )
        .unwrap();
    device
        .bind_image_memory(texture_image, texture_memory, 0)
        .unwrap();

    let cmd_pool = device
        .create_command_pool(
            &vk::CommandPoolCreateInfo {
                queue_family_index,
                flags: vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER,
                ..Default::default()
            },
            None,
        )
        .unwrap();
    let cmd = device
        .allocate_command_buffers(&vk::CommandBufferAllocateInfo {
            command_pool: cmd_pool,
            level: vk::CommandBufferLevel::PRIMARY,
            command_buffer_count: 1,
            ..Default::default()
        })
        .unwrap()[0];
    device
        .begin_command_buffer(cmd, &vk::CommandBufferBeginInfo::default())
        .unwrap();

    let barrier1 = vk::ImageMemoryBarrier {
        old_layout: vk::ImageLayout::UNDEFINED,
        new_layout: vk::ImageLayout::TRANSFER_DST_OPTIMAL,
        src_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
        dst_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
        image: texture_image,
        subresource_range: vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            level_count: 1,
            layer_count: 1,
            ..Default::default()
        },
        src_access_mask: vk::AccessFlags::empty(),
        dst_access_mask: vk::AccessFlags::TRANSFER_WRITE,
        ..Default::default()
    };
    device.cmd_pipeline_barrier(
        cmd,
        vk::PipelineStageFlags::TOP_OF_PIPE,
        vk::PipelineStageFlags::TRANSFER,
        vk::DependencyFlags::empty(),
        &[],
        &[],
        &[barrier1],
    );

    let staging_info = vk::BufferCreateInfo {
        size: atlas_data.len() as u64,
        usage: vk::BufferUsageFlags::TRANSFER_SRC,
        sharing_mode: vk::SharingMode::EXCLUSIVE,
        ..Default::default()
    };
    let staging_buffer = device.create_buffer(&staging_info, None).unwrap();
    let staging_reqs = device.get_buffer_memory_requirements(staging_buffer);
    let staging_idx = (0.._mem_props.memory_type_count)
        .find(|&i| {
            (staging_reqs.memory_type_bits & (1 << i)) != 0
                && _mem_props.memory_types[i as usize].property_flags.contains(
                    vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
                )
        })
        .unwrap();
    let staging_mem = device
        .allocate_memory(
            &vk::MemoryAllocateInfo {
                allocation_size: staging_reqs.size,
                memory_type_index: staging_idx,
                ..Default::default()
            },
            None,
        )
        .unwrap();
    device
        .bind_buffer_memory(staging_buffer, staging_mem, 0)
        .unwrap();
    let ptr = device
        .map_memory(
            staging_mem,
            0,
            atlas_data.len() as u64,
            vk::MemoryMapFlags::empty(),
        )
        .unwrap();
    std::ptr::copy_nonoverlapping(atlas_data.as_ptr(), ptr as *mut u8, atlas_data.len());
    device.unmap_memory(staging_mem);

    let region = vk::BufferImageCopy {
        buffer_offset: 0,
        buffer_row_length: 0,
        buffer_image_height: 0,
        image_subresource: vk::ImageSubresourceLayers {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            mip_level: 0,
            base_array_layer: 0,
            layer_count: 1,
        },
        image_offset: vk::Offset3D { x: 0, y: 0, z: 0 },
        image_extent: vk::Extent3D {
            width: ATLAS_WIDTH,
            height: ATLAS_HEIGHT,
            depth: 1,
        },
    };
    device.cmd_copy_buffer_to_image(
        cmd,
        staging_buffer,
        texture_image,
        vk::ImageLayout::TRANSFER_DST_OPTIMAL,
        &[region],
    );

    let barrier2 = vk::ImageMemoryBarrier {
        old_layout: vk::ImageLayout::TRANSFER_DST_OPTIMAL,
        new_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        src_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
        dst_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
        image: texture_image,
        subresource_range: vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            level_count: 1,
            layer_count: 1,
            ..Default::default()
        },
        src_access_mask: vk::AccessFlags::TRANSFER_WRITE,
        dst_access_mask: vk::AccessFlags::SHADER_READ,
        ..Default::default()
    };
    device.cmd_pipeline_barrier(
        cmd,
        vk::PipelineStageFlags::TRANSFER,
        vk::PipelineStageFlags::FRAGMENT_SHADER,
        vk::DependencyFlags::empty(),
        &[],
        &[],
        &[barrier2],
    );

    let view_info = vk::ImageViewCreateInfo {
        image: texture_image,
        view_type: vk::ImageViewType::TYPE_2D,
        format: vk::Format::R8_UNORM,
        subresource_range: vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            level_count: 1,
            layer_count: 1,
            ..Default::default()
        },
        ..Default::default()
    };
    let texture_view = device.create_image_view(&view_info, None).unwrap();
    let sampler = device
        .create_sampler(
            &vk::SamplerCreateInfo {
                mag_filter: vk::Filter::NEAREST,
                min_filter: vk::Filter::NEAREST,
                ..Default::default()
            },
            None,
        )
        .unwrap();

    // One mapped buffer holds six vertices for every cell. The frame fence is
    // waited before each update, so the GPU never reads data while it changes.
    let (terminal_columns, terminal_rows) = {
        let terminal = TERMINAL.lock().unwrap();
        (terminal.columns(), terminal.rows())
    };
    let vertex_capacity = terminal_columns * terminal_rows * 6;
    let vbuf_size = (vertex_capacity * std::mem::size_of::<Vertex>()) as u64;
    let vbuf_info = vk::BufferCreateInfo {
        size: vbuf_size,
        usage: vk::BufferUsageFlags::VERTEX_BUFFER,
        sharing_mode: vk::SharingMode::EXCLUSIVE,
        ..Default::default()
    };
    let vertex_buffer = device.create_buffer(&vbuf_info, None).unwrap();
    let vbuf_reqs = device.get_buffer_memory_requirements(vertex_buffer);
    let vertex_mem_type = (0.._mem_props.memory_type_count)
        .find(|&index| {
            (vbuf_reqs.memory_type_bits & (1 << index)) != 0
                && _mem_props.memory_types[index as usize]
                    .property_flags
                    .contains(
                        vk::MemoryPropertyFlags::HOST_VISIBLE
                            | vk::MemoryPropertyFlags::HOST_COHERENT,
                    )
        })
        .expect("No host-visible coherent Vulkan memory for terminal vertices");
    let vertex_buffer_memory = device
        .allocate_memory(
            &vk::MemoryAllocateInfo {
                allocation_size: vbuf_reqs.size,
                memory_type_index: vertex_mem_type,
                ..Default::default()
            },
            None,
        )
        .unwrap();
    device
        .bind_buffer_memory(vertex_buffer, vertex_buffer_memory, 0)
        .unwrap();

    device.end_command_buffer(cmd).unwrap();

    let upload_fence = device
        .create_fence(&vk::FenceCreateInfo::default(), None)
        .unwrap();
    device
        .queue_submit(
            queue,
            &[vk::SubmitInfo {
                command_buffer_count: 1,
                p_command_buffers: &cmd,
                ..Default::default()
            }],
            upload_fence,
        )
        .unwrap();
    device
        .wait_for_fences(&[upload_fence], true, u64::MAX)
        .unwrap();
    device.destroy_fence(upload_fence, None);
    device.destroy_buffer(staging_buffer, None);
    device.free_memory(staging_mem, None);

    let mapped_vertex_ptr = device
        .map_memory(
            vertex_buffer_memory,
            0,
            vbuf_reqs.size,
            vk::MemoryMapFlags::empty(),
        )
        .unwrap();

    let pool_info = vk::DescriptorPoolCreateInfo {
        max_sets: 1,
        pool_size_count: 1,
        p_pool_sizes: &vk::DescriptorPoolSize {
            ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
            descriptor_count: 1,
        },
        ..Default::default()
    };
    let descriptor_pool = device.create_descriptor_pool(&pool_info, None).unwrap();
    let layout_binding = vk::DescriptorSetLayoutBinding {
        binding: 0,
        descriptor_type: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
        descriptor_count: 1,
        stage_flags: vk::ShaderStageFlags::FRAGMENT,
        ..Default::default()
    };
    let descriptor_layout = device
        .create_descriptor_set_layout(
            &vk::DescriptorSetLayoutCreateInfo {
                binding_count: 1,
                p_bindings: &layout_binding,
                ..Default::default()
            },
            None,
        )
        .unwrap();
    let descriptor_sets = device
        .allocate_descriptor_sets(&vk::DescriptorSetAllocateInfo {
            descriptor_pool,
            descriptor_set_count: 1,
            p_set_layouts: &descriptor_layout,
            ..Default::default()
        })
        .unwrap();
    let img_desc = vk::DescriptorImageInfo {
        sampler,
        image_view: texture_view,
        image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
    };
    device.update_descriptor_sets(
        &[vk::WriteDescriptorSet {
            dst_set: descriptor_sets[0],
            dst_binding: 0,
            descriptor_count: 1,
            descriptor_type: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
            p_image_info: &img_desc,
            ..Default::default()
        }],
        &[],
    );

    let mut vert_words = vec![0u32; VERT_SPV.len().div_ceil(4)];
    let mut frag_words = vec![0u32; FRAG_SPV.len().div_ceil(4)];
    std::ptr::copy_nonoverlapping(
        VERT_SPV.as_ptr(),
        vert_words.as_mut_ptr() as *mut u8,
        VERT_SPV.len(),
    );
    std::ptr::copy_nonoverlapping(
        FRAG_SPV.as_ptr(),
        frag_words.as_mut_ptr() as *mut u8,
        FRAG_SPV.len(),
    );

    let vert_module = device
        .create_shader_module(
            &vk::ShaderModuleCreateInfo {
                code_size: vert_words.len() * 4,
                p_code: vert_words.as_ptr(),
                ..Default::default()
            },
            None,
        )
        .unwrap();
    let frag_module = device
        .create_shader_module(
            &vk::ShaderModuleCreateInfo {
                code_size: frag_words.len() * 4,
                p_code: frag_words.as_ptr(),
                ..Default::default()
            },
            None,
        )
        .unwrap();

    let main_name = CString::new("main").unwrap();
    let stages = [
        vk::PipelineShaderStageCreateInfo {
            stage: vk::ShaderStageFlags::VERTEX,
            module: vert_module,
            p_name: main_name.as_ptr(),
            ..Default::default()
        },
        vk::PipelineShaderStageCreateInfo {
            stage: vk::ShaderStageFlags::FRAGMENT,
            module: frag_module,
            p_name: main_name.as_ptr(),
            ..Default::default()
        },
    ];
    let binding_desc = [vk::VertexInputBindingDescription {
        binding: 0,
        stride: std::mem::size_of::<Vertex>() as u32,
        input_rate: vk::VertexInputRate::VERTEX,
    }];
    let attr_desc = [
        vk::VertexInputAttributeDescription {
            location: 0,
            binding: 0,
            format: vk::Format::R32G32_SFLOAT,
            offset: 0,
        },
        vk::VertexInputAttributeDescription {
            location: 1,
            binding: 0,
            format: vk::Format::R32G32_SFLOAT,
            offset: 8,
        },
        vk::VertexInputAttributeDescription {
            location: 2,
            binding: 0,
            format: vk::Format::R32G32B32A32_SFLOAT,
            offset: 16,
        },
        vk::VertexInputAttributeDescription {
            location: 3,
            binding: 0,
            format: vk::Format::R32G32B32A32_SFLOAT,
            offset: 32,
        },
    ];
    let vertex_input = vk::PipelineVertexInputStateCreateInfo {
        vertex_binding_description_count: 1,
        p_vertex_binding_descriptions: binding_desc.as_ptr(),
        vertex_attribute_description_count: attr_desc.len() as u32,
        p_vertex_attribute_descriptions: attr_desc.as_ptr(),
        ..Default::default()
    };

    // Push constant size is now 64 bytes (mat4)
    let push_constant_range = vk::PushConstantRange {
        stage_flags: vk::ShaderStageFlags::VERTEX,
        offset: 0,
        size: 64,
    };
    let pipeline_layout_info = vk::PipelineLayoutCreateInfo {
        set_layout_count: 1,
        p_set_layouts: &descriptor_layout,
        push_constant_range_count: 1,
        p_push_constant_ranges: &push_constant_range,
        ..Default::default()
    };
    let pipeline_layout = device
        .create_pipeline_layout(&pipeline_layout_info, None)
        .unwrap();

    let viewport = [vk::Viewport {
        x: 0.0,
        y: 0.0,
        width: width as f32,
        height: height as f32,
        min_depth: 0.0,
        max_depth: 1.0,
    }];
    let scissor = [vk::Rect2D {
        offset: vk::Offset2D { x: 0, y: 0 },
        extent: vk::Extent2D { width, height },
    }];
    let viewport_state = vk::PipelineViewportStateCreateInfo {
        viewport_count: 1,
        p_viewports: viewport.as_ptr(),
        scissor_count: 1,
        p_scissors: scissor.as_ptr(),
        ..Default::default()
    };

    let pipeline_info = vk::GraphicsPipelineCreateInfo {
        stage_count: 2,
        p_stages: stages.as_ptr(),
        p_vertex_input_state: &vertex_input,
        p_viewport_state: &viewport_state,
        p_input_assembly_state: &vk::PipelineInputAssemblyStateCreateInfo {
            topology: vk::PrimitiveTopology::TRIANGLE_LIST,
            ..Default::default()
        },
        p_rasterization_state: &vk::PipelineRasterizationStateCreateInfo {
            line_width: 1.0,
            cull_mode: vk::CullModeFlags::NONE,
            front_face: vk::FrontFace::COUNTER_CLOCKWISE,
            ..Default::default()
        },
        p_multisample_state: &vk::PipelineMultisampleStateCreateInfo {
            rasterization_samples: vk::SampleCountFlags::TYPE_1,
            ..Default::default()
        },
        p_color_blend_state: &vk::PipelineColorBlendStateCreateInfo {
            attachment_count: 1,
            p_attachments: &vk::PipelineColorBlendAttachmentState {
                blend_enable: vk::TRUE,
                src_color_blend_factor: vk::BlendFactor::SRC_ALPHA,
                dst_color_blend_factor: vk::BlendFactor::ONE_MINUS_SRC_ALPHA,
                color_write_mask: vk::ColorComponentFlags::RGBA,
                ..Default::default()
            },
            ..Default::default()
        },
        layout: pipeline_layout,
        render_pass,
        subpass: 0,
        ..Default::default()
    };
    let pipeline = device
        .create_graphics_pipelines(vk::PipelineCache::null(), &[pipeline_info], None)
        .unwrap()[0];

    let sem_avail = device
        .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
        .unwrap();
    let sem_finish: Vec<_> = (0..images.len())
        .map(|_| {
            device
                .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
                .unwrap()
        })
        .collect();
    let fence = device
        .create_fence(
            &vk::FenceCreateInfo {
                flags: vk::FenceCreateFlags::SIGNALED,
                ..Default::default()
            },
            None,
        )
        .unwrap();

    // --- Create Projection Matrix with m2s2-math ---
    // We construct the orthographic matrix manually to ensure exact Vulkan compatibility.
    // Maps X: 0 to width -> -1 to 1
    // Maps Y: 0 to height -> -1 to 1 (Vulkan NDC has Y pointing down, so 0 is top)
    let w = width as f32;
    let h = height as f32;

    // Row-major format. Surface pre-rotation is applied in clip space because
    // the swapchain preTransform matches the Android surface currentTransform.
    let orthographic_row_major = Matrix4x4f32::from_slice(&[
        2.0 / w,
        0.0,
        0.0,
        -1.0,
        0.0,
        2.0 / h,
        0.0,
        -1.0,
        0.0,
        0.0,
        1.0,
        0.0,
        0.0,
        0.0,
        0.0,
        1.0,
    ]);
    let proj_row_major = surface_pre_rotation(caps.current_transform) * orthographic_row_major;

    // Transpose to column-major for Vulkan
    let proj = proj_row_major.transpose();

    let proj_bytes = std::slice::from_raw_parts(
        &proj as *const _ as *const u8,
        64, // 4x4 * 4 bytes = 64
    );

    log::info!("Entering render loop (Bitmap Terminal with m2s2-math).");
    let mut vertices = Vec::with_capacity(vertex_capacity);

    while !SHOULD_CLOSE.load(Ordering::SeqCst) {
        device.wait_for_fences(&[fence], true, u64::MAX).unwrap();

        {
            let terminal = TERMINAL.lock().unwrap();
            build_grid_vertices_into(&terminal, width, height, &mut vertices);
        }
        if vertices.len() > vertex_capacity {
            log::error!("Terminal grid exceeds the allocated Vulkan vertex buffer");
            break;
        }
        std::ptr::copy_nonoverlapping(
            vertices.as_ptr(),
            mapped_vertex_ptr as *mut Vertex,
            vertices.len(),
        );

        device.reset_fences(&[fence]).unwrap();
        let acquire =
            swapchain_loader.acquire_next_image(swapchain, u64::MAX, sem_avail, vk::Fence::null());
        if acquire.is_err() {
            break;
        }
        let (idx, _) = acquire.unwrap();

        device
            .reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty())
            .unwrap();
        device
            .begin_command_buffer(cmd, &vk::CommandBufferBeginInfo::default())
            .unwrap();

        let clear = [vk::ClearValue {
            color: vk::ClearColorValue {
                float32: [0.0, 0.0, 0.0, 1.0],
            },
        }];
        device.cmd_begin_render_pass(
            cmd,
            &vk::RenderPassBeginInfo {
                render_pass,
                framebuffer: framebuffers[idx as usize],
                render_area: vk::Rect2D {
                    offset: vk::Offset2D { x: 0, y: 0 },
                    extent: vk::Extent2D { width, height },
                },
                clear_value_count: 1,
                p_clear_values: clear.as_ptr(),
                ..Default::default()
            },
            vk::SubpassContents::INLINE,
        );

        device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, pipeline);
        device.cmd_bind_vertex_buffers(cmd, 0, &[vertex_buffer], &[0]);
        device.cmd_bind_descriptor_sets(
            cmd,
            vk::PipelineBindPoint::GRAPHICS,
            pipeline_layout,
            0,
            &[descriptor_sets[0]],
            &[],
        );
        device.cmd_push_constants(
            cmd,
            pipeline_layout,
            vk::ShaderStageFlags::VERTEX,
            0,
            proj_bytes,
        );
        device.cmd_draw(cmd, vertices.len() as u32, 1, 0, 0);

        device.cmd_end_render_pass(cmd);
        device.end_command_buffer(cmd).unwrap();

        let wait_stages = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
        let submit = device.queue_submit(
            queue,
            &[vk::SubmitInfo {
                wait_semaphore_count: 1,
                p_wait_semaphores: &sem_avail,
                p_wait_dst_stage_mask: wait_stages.as_ptr(),
                command_buffer_count: 1,
                p_command_buffers: &cmd,
                signal_semaphore_count: 1,
                p_signal_semaphores: &sem_finish[idx as usize],
                ..Default::default()
            }],
            fence,
        );
        if submit.is_err() {
            break;
        }

        let present = swapchain_loader.queue_present(
            queue,
            &vk::PresentInfoKHR {
                wait_semaphore_count: 1,
                p_wait_semaphores: &sem_finish[idx as usize],
                swapchain_count: 1,
                p_swapchains: &swapchain,
                p_image_indices: &idx,
                ..Default::default()
            },
        );
        if present.is_err() {
            break;
        }
    }

    let _ = device.device_wait_idle();
    device.unmap_memory(vertex_buffer_memory);
    device.destroy_fence(fence, None);
    device.destroy_semaphore(sem_avail, None);
    for semaphore in sem_finish {
        device.destroy_semaphore(semaphore, None);
    }
    device.destroy_pipeline(pipeline, None);
    device.destroy_shader_module(vert_module, None);
    device.destroy_shader_module(frag_module, None);
    device.destroy_pipeline_layout(pipeline_layout, None);
    device.destroy_descriptor_pool(descriptor_pool, None);
    device.destroy_descriptor_set_layout(descriptor_layout, None);
    device.destroy_sampler(sampler, None);
    device.destroy_image_view(texture_view, None);
    device.destroy_image(texture_image, None);
    device.free_memory(texture_memory, None);
    device.destroy_buffer(vertex_buffer, None);
    device.free_memory(vertex_buffer_memory, None);
    for framebuffer in framebuffers {
        device.destroy_framebuffer(framebuffer, None);
    }
    for view in swapchain_image_views {
        device.destroy_image_view(view, None);
    }
    device.destroy_render_pass(render_pass, None);
    device.destroy_command_pool(cmd_pool, None);
    swapchain_loader.destroy_swapchain(swapchain, None);
    device.destroy_device(None);
    surface_ext.destroy_surface(surface, None);
    instance.destroy_instance(None);
}
