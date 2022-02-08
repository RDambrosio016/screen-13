use {
    super::{format_aspect_mask, Device, DriverError},
    crate::ptr::Shared,
    archery::SharedPointerKind,
    ash::vk,
    derive_builder::Builder,
    glam::{uvec3, UVec2, UVec3, Vec4},
    gpu_allocator::{
        vulkan::{Allocation, AllocationCreateDesc},
        MemoryLocation,
    },
    log::{trace, warn},
    parking_lot::Mutex,
    std::{
        collections::{hash_map::Entry, HashMap},
        ops::Deref,
        ptr::null,
        thread::panicking,
    },
};

#[derive(Debug)]
pub struct Image<P>
where
    P: SharedPointerKind,
{
    pub allocation: Option<Allocation>, // None when we don't own the image (Swapchain images)
    device: Shared<Device<P>, P>,
    image: vk::Image,
    #[allow(clippy::type_complexity)]
    image_view_cache: Shared<Mutex<HashMap<ImageViewInfo, ImageView<P>>>, P>,
    pub info: ImageInfo,
}

impl<P> Image<P>
where
    P: SharedPointerKind,
{
    pub fn create(
        device: &Shared<Device<P>, P>,
        info: impl Into<ImageInfo>,
    ) -> Result<Self, DriverError> {
        let mut info: ImageInfo = info.into();

        //trace!("create: {:?}", &info);
        trace!("create");

        assert!(
            !info.usage.is_empty(),
            "Unspecified image usage {:?}",
            info.usage
        );

        if info.usage.contains(vk::ImageUsageFlags::COLOR_ATTACHMENT)
            && !device
                .physical_device
                .props
                .limits
                .framebuffer_color_sample_counts
                .contains(info.sample_count.into_vk())
        {
            info.sample_count = info
                .sample_count
                .compatible_items()
                .find(|sample_count| {
                    device
                        .physical_device
                        .props
                        .limits
                        .framebuffer_color_sample_counts
                        .contains(sample_count.into_vk())
                })
                .unwrap();
        }

        if info
            .usage
            .contains(vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT)
            && !device
                .physical_device
                .props
                .limits
                .framebuffer_depth_sample_counts
                .contains(info.sample_count.into_vk())
        {
            info.sample_count = info
                .sample_count
                .compatible_items()
                .find(|sample_count| {
                    device
                        .physical_device
                        .props
                        .limits
                        .framebuffer_depth_sample_counts
                        .contains(sample_count.into_vk())
                })
                .unwrap();
        }

        let device = Shared::clone(device);
        let create_info = info.image_create_info();
        let image = unsafe {
            device
                .create_image(&create_info, None)
                .map_err(|_| DriverError::Unsupported)?
        };
        let requirements = unsafe { device.get_image_memory_requirements(image) };
        let allocation = device
            .allocator
            .as_ref()
            .unwrap()
            .lock()
            .allocate(&AllocationCreateDesc {
                name: "image",
                requirements,
                location: MemoryLocation::GpuOnly,
                linear: false,
            })
            .map_err(|_| DriverError::Unsupported)?;

        unsafe {
            device
                .bind_image_memory(image, allocation.memory(), allocation.offset())
                .map_err(|_| DriverError::Unsupported)?;
        }

        Ok(Self {
            allocation: Some(allocation),
            device,
            image,
            image_view_cache: Shared::new(Mutex::new(Default::default())),
            info,
        })
    }

    /// Suprisingly this isn't at all dangerous but it may not be what you want
    pub(super) fn clone_raw(this: &Self) -> Self {
        Self {
            allocation: None,
            device: Shared::clone(&this.device),
            image: this.image,
            image_view_cache: Shared::new(Mutex::new(Default::default())),
            info: this.info,
        }
    }

    pub fn create_view(this: &Self, info: ImageViewInfo) -> Result<ImageView<P>, DriverError> {
        ImageView::create(&this.device, info, this)
    }

    pub fn from_raw(device: &Shared<Device<P>, P>, image: vk::Image, info: ImageInfo) -> Self {
        let device = Shared::clone(device);

        Self {
            allocation: None,
            device,
            image,
            image_view_cache: Shared::new(Mutex::new(Default::default())),
            info,
        }
    }

    pub fn view_ref(this: &Self, info: ImageViewInfo) -> Result<vk::ImageView, DriverError> {
        let mut image_view_cache = this.image_view_cache.lock();

        Ok(match image_view_cache.entry(info) {
            Entry::Occupied(entry) => **entry.get(),
            Entry::Vacant(entry) => **entry.insert(Self::create_view(this, info)?),
        })
    }
}

impl<P> Deref for Image<P>
where
    P: SharedPointerKind,
{
    type Target = vk::Image;

    fn deref(&self) -> &Self::Target {
        &self.image
    }
}

impl<P> Drop for Image<P>
where
    P: SharedPointerKind,
{
    fn drop(&mut self) {
        if panicking() {
            return;
        }

        self.image_view_cache.lock().clear();

        // When our allocation is some we allocated ourself; otherwise somebody
        // else owns this image and we should not destroy it. Usually it's the swapchain...
        if let Some(allocation) = self.allocation.take() {
            unsafe {
                self.device.destroy_image(self.image, None);
            }

            self.device
                .allocator
                .as_ref()
                .unwrap()
                .lock()
                .free(allocation)
                .unwrap_or_else(|_| warn!("Unable to free image allocation"));
        }
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum ImageType {
    Texture1D = 0,
    TextureArray1D = 1,
    Texture2D = 2,
    TextureArray2D = 3,
    Texture3D = 4,
    Cube = 5,
    CubeArray = 6,
}

impl ImageType {
    pub fn into_vk(self) -> vk::ImageViewType {
        match self {
            Self::Cube => vk::ImageViewType::CUBE,
            Self::CubeArray => vk::ImageViewType::CUBE_ARRAY,
            Self::Texture1D => vk::ImageViewType::TYPE_1D,
            Self::Texture2D => vk::ImageViewType::TYPE_2D,
            Self::Texture3D => vk::ImageViewType::TYPE_3D,
            Self::TextureArray1D => vk::ImageViewType::TYPE_1D_ARRAY,
            Self::TextureArray2D => vk::ImageViewType::TYPE_2D_ARRAY,
        }
    }
}

#[derive(Builder, Clone, Copy, Debug, Hash, PartialEq, Eq)]
#[builder(pattern = "owned", derive(Debug))]
pub struct ImageInfo {
    #[builder(setter(strip_option))]
    pub ty: ImageType,
    #[builder(default, setter(strip_option))]
    pub usage: vk::ImageUsageFlags,
    #[builder(default, setter(strip_option))]
    pub flags: vk::ImageCreateFlags,
    #[builder(setter(strip_option))]
    pub fmt: vk::Format,
    #[builder(setter(strip_option))]
    pub extent: UVec3,
    #[builder(default = "vk::ImageTiling::OPTIMAL", setter(strip_option))]
    pub tiling: vk::ImageTiling,
    #[builder(default = "1", setter(strip_option))]
    pub mip_level_count: u32,
    #[builder(default = "1", setter(strip_option))]
    pub array_elements: u32,
    #[builder(default = "SampleCount::X1", setter(strip_option))]
    pub sample_count: SampleCount,
}

impl ImageInfo {
    #[allow(clippy::new_ret_no_self)]
    fn new(fmt: vk::Format, ty: ImageType, extent: UVec3) -> ImageInfoBuilder {
        ImageInfoBuilder {
            ty: Some(ty),
            fmt: Some(fmt),
            extent: Some(extent),
            ..Default::default()
        }
    }

    pub fn new_1d(fmt: vk::Format, extent: u32) -> ImageInfoBuilder {
        Self::new(fmt, ImageType::Texture1D, uvec3(extent, 1, 1))
    }

    pub fn new_2d(fmt: vk::Format, extent: UVec2) -> ImageInfoBuilder {
        Self::new(fmt, ImageType::Texture2D, uvec3(extent.x, extent.y, 1))
    }

    pub fn new_3d(fmt: vk::Format, extent: UVec3) -> ImageInfoBuilder {
        Self::new(fmt, ImageType::Texture3D, extent)
    }

    pub fn new_cube(fmt: vk::Format, width: u32) -> ImageInfoBuilder {
        ImageInfoBuilder::default()
            .fmt(fmt)
            .ty(ImageType::Cube)
            .extent(uvec3(width, width, 1))
            .array_elements(6)
            .flags(vk::ImageCreateFlags::CUBE_COMPATIBLE)
    }

    fn image_create_info(self) -> vk::ImageCreateInfo {
        let (ty, extent, array_layers) = match self.ty {
            ImageType::Texture1D => (
                vk::ImageType::TYPE_1D,
                vk::Extent3D {
                    width: self.extent.x,
                    height: 1,
                    depth: 1,
                },
                1,
            ),
            ImageType::TextureArray1D => (
                vk::ImageType::TYPE_1D,
                vk::Extent3D {
                    width: self.extent.x,
                    height: 1,
                    depth: 1,
                },
                self.array_elements,
            ),
            ImageType::Texture2D => (
                vk::ImageType::TYPE_2D,
                vk::Extent3D {
                    width: self.extent.x,
                    height: self.extent.y,
                    depth: 1,
                },
                1,
            ),
            ImageType::TextureArray2D => (
                vk::ImageType::TYPE_2D,
                vk::Extent3D {
                    width: self.extent.x,
                    height: self.extent.y,
                    depth: 1,
                },
                self.array_elements,
            ),
            ImageType::Texture3D => (
                vk::ImageType::TYPE_3D,
                vk::Extent3D {
                    width: self.extent.x,
                    height: self.extent.y,
                    depth: self.extent.z,
                },
                1,
            ),
            ImageType::Cube => (
                vk::ImageType::TYPE_2D,
                vk::Extent3D {
                    width: self.extent.x,
                    height: self.extent.y,
                    depth: 1,
                },
                6,
            ),
            ImageType::CubeArray => (
                vk::ImageType::TYPE_2D,
                vk::Extent3D {
                    width: self.extent.x,
                    height: self.extent.y,
                    depth: 1,
                },
                6 * self.array_elements,
            ),
        };

        vk::ImageCreateInfo {
            flags: self.flags,
            image_type: ty,
            format: self.fmt,
            extent,
            mip_levels: self.mip_level_count,
            array_layers,
            samples: self.sample_count.into_vk(),
            tiling: self.tiling,
            usage: self.usage,
            sharing_mode: vk::SharingMode::EXCLUSIVE,
            initial_layout: vk::ImageLayout::UNDEFINED,
            ..Default::default()
        }
    }

    pub fn extent_inv_extent_2d(&self) -> Vec4 {
        let extent = self.extent.as_vec3();
        Vec4::new(extent.x, extent.y, 1.0 / extent.x, 1.0 / extent.y)
    }

    pub fn extent_2d(self) -> UVec2 {
        self.extent.truncate()
    }

    pub fn fmt(mut self, fmt: vk::Format) -> Self {
        self.fmt = fmt;
        self
    }

    pub fn into_builder(self) -> ImageInfoBuilder {
        ImageInfoBuilder {
            array_elements: Some(self.array_elements),
            extent: Some(self.extent),
            flags: Some(self.flags),
            fmt: Some(self.fmt),
            mip_level_count: Some(self.mip_level_count),
            sample_count: None,
            tiling: Some(self.tiling),
            ty: Some(self.ty),
            usage: Some(self.usage),
        }
    }
}

impl ImageInfoBuilder {
    pub fn all_mip_levels(self) -> Self {
        assert!(self.extent.is_some());

        let extent = self.extent.unwrap();

        self.mip_level_count(
            Self::mip_count_1d(extent.x)
                .max(Self::mip_count_1d(extent.y).max(Self::mip_count_1d(extent.z))),
        )
    }

    pub fn extent_div(mut self, denom: UVec3) -> Self {
        assert!(self.extent.is_some());

        self.extent = Some(self.extent.unwrap() / denom);
        self
    }

    pub fn extent_div_up(mut self, denom: UVec3) -> Self {
        assert!(self.extent.is_some());

        self.extent = Some((self.extent.unwrap() + denom - UVec3::ONE) / denom);
        self
    }

    pub fn half_res(self) -> Self {
        self.extent_div_up(uvec3(2, 2, 2))
    }

    fn mip_count_1d(extent: u32) -> u32 {
        32 - extent.leading_zeros()
    }
}

impl From<ImageInfoBuilder> for ImageInfo {
    fn from(info: ImageInfoBuilder) -> Self {
        info.build().unwrap()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImageSubresource {
    pub array_layer_count: Option<u32>,
    pub aspect_mask: vk::ImageAspectFlags,
    pub base_array_layer: u32,
    pub base_mip_level: u32,
    pub mip_level_count: Option<u32>,
}

impl ImageSubresource {
    pub fn into_vk(self) -> vk::ImageSubresourceRange {
        vk::ImageSubresourceRange {
            aspect_mask: self.aspect_mask,
            base_mip_level: self.base_mip_level,
            base_array_layer: self.base_array_layer,
            layer_count: self.array_layer_count.unwrap_or(vk::REMAINING_ARRAY_LAYERS),
            level_count: self.mip_level_count.unwrap_or(vk::REMAINING_MIP_LEVELS),
        }
    }
}

impl Default for ImageSubresource {
    fn default() -> Self {
        Self {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            base_array_layer: 0,
            array_layer_count: None,
            mip_level_count: None,
        }
    }
}

impl From<ImageInfo> for ImageSubresource {
    fn from(info: ImageInfo) -> Self {
        Self {
            aspect_mask: format_aspect_mask(info.fmt),
            ..Default::default()
        }
    }
}

impl From<ImageViewInfo> for ImageSubresource {
    fn from(info: ImageViewInfo) -> Self {
        Self {
            aspect_mask: info.aspect_mask,
            base_mip_level: info.base_mip_level,
            base_array_layer: info.base_array_layer,
            array_layer_count: Some(match info.ty {
                ImageType::Texture1D | ImageType::Texture2D | ImageType::Texture3D => 1,
                ImageType::Cube | ImageType::CubeArray => 6,
                ImageType::TextureArray1D | ImageType::TextureArray2D => 1,
            }),
            mip_level_count: info.mip_level_count,
        }
    }
}

#[derive(Debug)]
pub struct ImageView<P>
where
    P: SharedPointerKind,
{
    device: Shared<Device<P>, P>,
    image_view: vk::ImageView,
    pub info: ImageViewInfo,
}

impl<P> ImageView<P>
where
    P: SharedPointerKind,
{
    pub fn create(
        device: &Shared<Device<P>, P>,
        info: impl Into<ImageViewInfo>,
        image: &Image<P>,
    ) -> Result<Self, DriverError> {
        let info = info.into();
        let device = Shared::clone(device);
        let create_info = vk::ImageViewCreateInfo {
            s_type: vk::StructureType::IMAGE_VIEW_CREATE_INFO,
            p_next: null(),
            flags: vk::ImageViewCreateFlags::empty(),
            view_type: info.ty.into_vk(),
            format: info.fmt,
            components: vk::ComponentMapping {
                r: vk::ComponentSwizzle::R,
                g: vk::ComponentSwizzle::G,
                b: vk::ComponentSwizzle::B,
                a: vk::ComponentSwizzle::A,
            },
            image: **image,
            subresource_range: vk::ImageSubresourceRange {
                aspect_mask: info.aspect_mask,
                base_array_layer: info.base_array_layer,
                base_mip_level: info.base_mip_level,
                level_count: info.mip_level_count.unwrap_or(vk::REMAINING_MIP_LEVELS),
                layer_count: info.array_layer_count.unwrap_or(vk::REMAINING_ARRAY_LAYERS),
            },
        };

        let image_view = unsafe { device.create_image_view(&create_info, None) }
            .map_err(|_| DriverError::Unsupported)?;

        Ok(Self {
            device,
            image_view,
            info,
        })
    }
}

impl<P> Deref for ImageView<P>
where
    P: SharedPointerKind,
{
    type Target = vk::ImageView;

    fn deref(&self) -> &Self::Target {
        &self.image_view
    }
}

impl<P> Drop for ImageView<P>
where
    P: SharedPointerKind,
{
    fn drop(&mut self) {
        if panicking() {
            return;
        }

        unsafe {
            self.device.destroy_image_view(self.image_view, None);
        }
    }
}

#[derive(Builder, Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[builder(pattern = "owned")]
pub struct ImageViewInfo {
    pub array_layer_count: Option<u32>,
    pub aspect_mask: vk::ImageAspectFlags,
    pub base_array_layer: u32,
    pub base_mip_level: u32,
    pub fmt: vk::Format,
    pub mip_level_count: Option<u32>,
    pub ty: ImageType,
}

impl ImageViewInfo {
    #[allow(clippy::new_ret_no_self)]
    pub fn new() -> ImageViewInfoBuilder {
        Default::default() // TODO: Maybe not default... need format?
    }
}

impl From<ImageInfo> for ImageViewInfo {
    fn from(info: ImageInfo) -> Self {
        Self {
            array_layer_count: Some(info.array_elements),
            aspect_mask: format_aspect_mask(info.fmt),
            base_array_layer: 0,
            base_mip_level: 0,
            fmt: info.fmt,
            mip_level_count: Some(info.mip_level_count),
            ty: info.ty,
        }
    }
}

impl From<ImageViewInfoBuilder> for ImageViewInfo {
    fn from(info: ImageViewInfoBuilder) -> Self {
        info.build().unwrap()
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SampleCount {
    X1,
    X2,
    X4,
    X8,
    X16,
    X32,
    X64,
}

impl SampleCount {
    pub fn compatible_items(self) -> impl Iterator<Item = Self> {
        SampleCountCompatibilityIter(self)
    }

    pub fn into_vk(self) -> vk::SampleCountFlags {
        match self {
            Self::X1 => vk::SampleCountFlags::TYPE_1,
            Self::X2 => vk::SampleCountFlags::TYPE_2,
            Self::X4 => vk::SampleCountFlags::TYPE_4,
            Self::X8 => vk::SampleCountFlags::TYPE_8,
            Self::X16 => vk::SampleCountFlags::TYPE_16,
            Self::X32 => vk::SampleCountFlags::TYPE_32,
            Self::X64 => vk::SampleCountFlags::TYPE_64,
        }
    }
}

impl Default for SampleCount {
    fn default() -> Self {
        Self::X1
    }
}

struct SampleCountCompatibilityIter(SampleCount);

impl Iterator for SampleCountCompatibilityIter {
    type Item = SampleCount;

    fn next(&mut self) -> Option<Self::Item> {
        Some(match self.0 {
            SampleCount::X1 => return None,
            SampleCount::X2 => SampleCount::X1,
            SampleCount::X4 => SampleCount::X2,
            SampleCount::X8 => SampleCount::X4,
            SampleCount::X16 => SampleCount::X8,
            SampleCount::X32 => SampleCount::X16,
            SampleCount::X64 => SampleCount::X32,
        })
    }
}
