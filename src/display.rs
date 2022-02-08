use {
    super::{
        driver::{
            CommandBuffer, Device, DriverError, ImageSubresource, Swapchain, SwapchainImageError,
        },
        graph::{RenderGraph, Resolver, SwapchainImageNode},
        ptr::Shared,
        HashPool,
    },
    archery::SharedPointerKind,
    ash::vk,
    log::trace,
    std::{
        collections::VecDeque,
        error::Error,
        fmt::Formatter,
        time::{Duration, Instant},
    },
    vk_sync::AccessType,
};

#[derive(Debug)]
pub struct Display<P>
where
    P: SharedPointerKind,
{
    cache: HashPool<P>,
    device: Shared<Device<P>, P>,
    frames: Vec<Frame<P>>,
    resolved: VecDeque<Resolver<P>>,
    swapchain: Swapchain<P>,
}

impl<P> Display<P>
where
    P: SharedPointerKind + 'static,
{
    pub fn new(device: &Shared<Device<P>, P>, swapchain: Swapchain<P>) -> Self {
        let device = Shared::clone(device);

        Self {
            cache: HashPool::new(&device),
            device,
            frames: Default::default(),
            resolved: Default::default(),
            swapchain,
        }
    }

    pub fn acquire_next_image(
        &mut self,
    ) -> Result<(SwapchainImageNode<P>, RenderGraph<P>), DisplayError>
    where
        P: 'static,
    {
        trace!("acquire_next_image");

        let swapchain_image = self.swapchain.acquire_next_image()?; // TODO: Rebuild swapchain and device wait_idle until it's fixed!
        let mut render_graph = RenderGraph::new();
        let swapchain = render_graph.bind_node(swapchain_image);

        Ok((swapchain, render_graph))
    }

    unsafe fn begin(&self, cmd_buf: &CommandBuffer<P>) -> Result<(), ()> {
        use std::slice::from_ref;

        Device::wait_for_fence(&self.device, &cmd_buf.fence).map_err(|_| ())?;

        self.device
            .reset_command_pool(cmd_buf.pool, vk::CommandPoolResetFlags::RELEASE_RESOURCES)
            .map_err(|_| ())?;
        self.device
            .begin_command_buffer(
                **cmd_buf,
                &vk::CommandBufferBeginInfo::builder()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )
            .map_err(|_| ())
    }

    pub fn present_image(
        &mut self,
        render_graph: RenderGraph<P>,
        swapchain_image: SwapchainImageNode<P>,
    ) -> Result<(), DisplayError> {
        use std::slice::from_ref;

        trace!("present_image");

        let (last_swapchain_access, _) = render_graph.last_access(swapchain_image).unwrap();
        let mut resolver = render_graph.resolve();
        let wait_dst_stage_mask = resolver.node_stage_mask(swapchain_image);
        let swapchain_node = swapchain_image;
        let swapchain_image = resolver.unbind_node(swapchain_node);
        let swapchain_image_idx = swapchain_image.idx as usize;

        while self.frames.len() <= swapchain_image_idx {
            self.frames.push(Frame {
                main_cmd_buf: CommandBuffer::create(&self.device, self.device.queue.family)?,
                presentation_cmd_buf: CommandBuffer::create(
                    &self.device,
                    self.device.queue.family,
                )?,
                resolved_render_graph: None,
            });
        }

        let frame = &self.frames[swapchain_image_idx];
        let started = Instant::now();

        // Record up to but not including the swapchain work
        {
            unsafe { self.begin(&frame.main_cmd_buf) }?;

            resolver.record_node_dependencies(
                &mut self.cache,
                &frame.main_cmd_buf,
                swapchain_node,
            )?;

            unsafe {
                self.submit(
                    &frame.main_cmd_buf,
                    vk::SubmitInfo::builder().command_buffers(from_ref(&frame.main_cmd_buf)),
                )
            }?;
        }

        let elapsed = Instant::now() - started;
        trace!("Node dependencies took {} μs", elapsed.as_micros());

        // Switch commnd buffers because we're going to be submitting with a wait semaphore on the
        // swapchain image before we get access to record commands that use it
        {
            unsafe { self.begin(&frame.presentation_cmd_buf) }?;

            resolver.record_node(&mut self.cache, &frame.presentation_cmd_buf, swapchain_node)?;

            CommandBuffer::image_barrier(
                &frame.presentation_cmd_buf,
                last_swapchain_access,
                AccessType::Present,
                **swapchain_image,
                Some(ImageSubresource {
                    array_layer_count: None,
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_array_layer: 0,
                    base_mip_level: 0,
                    mip_level_count: None,
                }),
            );

            unsafe {
                self.submit(
                    &frame.presentation_cmd_buf,
                    vk::SubmitInfo::builder()
                        .command_buffers(from_ref(&frame.presentation_cmd_buf))
                        .signal_semaphores(from_ref(&swapchain_image.rendered))
                        .wait_semaphores(from_ref(&swapchain_image.acquired))
                        .wait_dst_stage_mask(from_ref(&wait_dst_stage_mask)),
                )
            }?;
        }

        let elapsed = Instant::now() - started;
        trace!("Command buffer recording total: {} μs", elapsed.as_micros());

        self.swapchain.present_image(swapchain_image);

        // Store the resolved graph because it contains bindings, leases, and other shared resources
        // that need to be kept alive until the fence is waited upon.
        let frame = &mut self.frames[swapchain_image_idx];
        frame.resolved_render_graph = Some(resolver);

        Ok(())
    }

    unsafe fn submit(
        &self,
        cmd_buf: &CommandBuffer<P>,
        submit_info: vk::SubmitInfoBuilder<'_>,
    ) -> Result<(), ()> {
        use std::slice::from_ref;

        self.device.end_command_buffer(**cmd_buf).map_err(|_| ())?;
        self.device
            .reset_fences(from_ref(&cmd_buf.fence))
            .map_err(|_| ())?;
        self.device
            .queue_submit(*self.device.queue, from_ref(&*submit_info), cmd_buf.fence)
            .map_err(|_| ())
    }
}

#[derive(Debug)]
pub enum DisplayError {
    /// Unrecoverable device error; must destroy this device and display and start a new one
    DeviceLost,

    /// Recoverable driver error
    Driver(DriverError),
}

impl Error for DisplayError {}

impl From<()> for DisplayError {
    fn from(_: ()) -> Self {
        Self::DeviceLost
    }
}

impl From<DriverError> for DisplayError {
    fn from(err: DriverError) -> Self {
        Self::Driver(err)
    }
}

impl From<SwapchainImageError> for DisplayError {
    fn from(err: SwapchainImageError) -> Self {
        Self::DeviceLost
    }
}

impl std::fmt::Display for DisplayError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

#[derive(Debug)]
struct Frame<P>
where
    P: SharedPointerKind,
{
    main_cmd_buf: CommandBuffer<P>,
    presentation_cmd_buf: CommandBuffer<P>,
    resolved_render_graph: Option<Resolver<P>>, // TODO: Only want the physical passes; could drop rest
}
