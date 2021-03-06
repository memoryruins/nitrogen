/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

//! Description of image objects.

use gfx::image;
use gfx::Device;

use std;
use std::borrow::Borrow;
use std::collections::BTreeSet;
use std::hash::{Hash, Hasher};

use crate::util::allocator::{AllocatorError, BufferRequest, Image as AllocImage, ImageRequest};
use crate::util::storage::{Handle, Storage};
use crate::util::transfer;

use crate::device::DeviceContext;
use crate::resources::command_pool::CommandPoolTransfer;
use crate::submit_group::{QueueSyncRefs, ResourceList};

/// Source channel for a `Swizzle`
#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Component {
    /// Hardcoded zero
    Zero,
    /// Hardcoded one
    One,
    /// Red channel
    R,
    /// Green channel
    G,
    /// Blue channel
    B,
    /// Alpha channel
    A,
}

/// Swizzles can be used for remapping components of a format.
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct Swizzle(pub Component, pub Component, pub Component, pub Component);

impl Swizzle {
    /// Swizzle configuration which maps all channels to themselves.
    pub const NO: Swizzle = Swizzle(Component::R, Component::G, Component::B, Component::A);
}

impl Default for Swizzle {
    fn default() -> Self {
        Swizzle::NO
    }
}

impl From<Component> for gfx::format::Component {
    fn from(c: Component) -> Self {
        match c {
            Component::Zero => gfx::format::Component::Zero,
            Component::One => gfx::format::Component::One,
            Component::R => gfx::format::Component::R,
            Component::G => gfx::format::Component::G,
            Component::B => gfx::format::Component::B,
            Component::A => gfx::format::Component::A,
        }
    }
}

impl From<Swizzle> for gfx::format::Swizzle {
    fn from(Swizzle(r, g, b, a): Swizzle) -> Self {
        gfx::format::Swizzle(r.into(), g.into(), b.into(), a.into())
    }
}

/// Dimensions of an image object.
#[derive(Copy, Clone, Debug)]
pub enum ImageDimension {
    /// Dimensions for a 1D-image.
    #[allow(missing_docs)]
    D1 { x: u32 },

    /// Dimensions for a 2D-image.
    #[allow(missing_docs)]
    D2 { x: u32, y: u32 },

    /// Dimensions for a 3D-image.
    #[allow(missing_docs)]
    D3 { x: u32, y: u32, z: u32 },
}

impl Default for ImageDimension {
    fn default() -> Self {
        ImageDimension::D2 { x: 1, y: 1 }
    }
}

impl ImageDimension {
    /// Calculate the "size-triple" (width, height, depth) of the image.
    ///
    /// In some situations, an "empty component" (for example depth when dealing with 2D images)
    /// should be filled with a certain value. The `fill` argument is be used to provide that value.
    pub fn as_triple(&self, fill: u32) -> (u32, u32, u32) {
        use self::ImageDimension::*;
        match self {
            D1 { x } => (*x, fill, fill),
            D2 { x, y } => (*x, *y, fill),
            D3 { x, y, z } => (*x, *y, *z),
        }
    }
}

/// Size mode used for image resources created in graphs.
#[derive(Debug, Clone, Copy)]
pub enum ImageSizeMode {
    /// The size of the image depends on the context reference size. See [`ExecutionContext`].
    ///
    /// [`ExecutionContext`]: ../graph/struct.ExecutionContext.html
    #[allow(missing_docs)]
    ContextRelative { width: f32, height: f32 },

    /// The size of the image is specified directly.
    #[allow(missing_docs)]
    Absolute { width: u32, height: u32 },
}

impl ImageSizeMode {
    ///
    pub fn absolute(&self, reference: (u32, u32)) -> (u32, u32) {
        match self {
            ImageSizeMode::ContextRelative { width, height } => (
                (f64::from(*width) * f64::from(reference.0)) as u32,
                (f64::from(*height) * f64::from(reference.1)) as u32,
            ),
            ImageSizeMode::Absolute { width, height } => (*width, *height),
        }
    }
}

impl Hash for ImageSizeMode {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            ImageSizeMode::ContextRelative { .. } => {
                state.write_i8(0);
            }
            ImageSizeMode::Absolute { width, height } => {
                state.write_i8(1);
                state.write_u32(*width);
                state.write_u32(*height);
            }
        }
    }
}

/// Opaque handle to an image object.
pub type ImageHandle = Handle<Image>;

/// Description of an image object's properties.
#[derive(Default, Clone)]
pub struct ImageCreateInfo<T: Into<gfx::image::Usage>> {
    /// Dimensions ("size") of the image.
    pub dimension: ImageDimension,
    /// Number of layers of the image. Used for image arrays and cube images.
    pub num_layers: u16,
    /// Number of samples used for multisampling.
    pub num_samples: u8,
    /// Number of mipmap images.
    pub num_mipmaps: u8,
    /// Format of the image.
    pub format: ImageFormat,
    /// "rewriting" of components when sampling.
    pub swizzle: Swizzle,
    /// Kind of image, this can affect internal memory layout and sampling behavior.
    pub kind: ViewKind,

    /// Usage flags of the image.
    pub usage: T,

    /// Flag to indicate whether the image object is short-lived or not.
    pub is_transient: bool,
}

/// Flags describing how an image object can be used.
#[allow(missing_docs)]
#[repr(C)]
#[derive(Default, Debug, Clone, Copy, Hash)]
pub struct ImageUsage {
    pub transfer_src: bool,
    pub transfer_dst: bool,
    pub sampling: bool,
    pub color_attachment: bool,
    pub depth_stencil_attachment: bool,
    pub storage_image: bool,
    pub input_attachment: bool,
}

impl From<ImageUsage> for gfx::image::Usage {
    fn from(val: ImageUsage) -> Self {
        use gfx::image::Usage;

        let mut flags = Usage::empty();

        if val.transfer_src {
            flags |= Usage::TRANSFER_SRC;
        }
        if val.transfer_dst {
            flags |= Usage::TRANSFER_DST;
        }

        if val.sampling {
            flags |= Usage::SAMPLED;
        }
        if val.color_attachment {
            flags |= Usage::COLOR_ATTACHMENT;
        }
        if val.depth_stencil_attachment {
            flags |= Usage::DEPTH_STENCIL_ATTACHMENT;
        }
        if val.storage_image {
            flags |= Usage::STORAGE;
        }
        if val.input_attachment {
            flags |= Usage::INPUT_ATTACHMENT;
        }

        flags
    }
}

/// Data provided for uploading data to an image object.
pub struct ImageUploadInfo<'a> {
    /// The data to be uploaded.
    pub data: &'a [u8],
    /// The format of the data. This has to match or be compatible with the format of the
    /// destination image.
    pub format: ImageFormat,
    /// The dimensions of the upload.
    ///
    /// The same data provided can be written to the destination image in different ways.
    /// For example a 2x2x2 3D upload will be written differently than a 4x2x1 upload, while having
    /// the same representation in memory.
    pub dimension: ImageDimension,
    /// Offset in the target image to write to.
    pub target_offset: (u32, u32, u32),
}

/// Image formats
#[repr(u8)]
#[allow(missing_docs)]
#[derive(Copy, Clone, Debug, PartialEq, Hash)]
pub enum ImageFormat {
    RUnorm,
    RgUnorm,
    RgbUnorm,
    RgbaUnorm,

    Rgba32Float,

    E5b9g9r9Float,

    D32Float,
    D32FloatS8Uint,
}

impl Default for ImageFormat {
    fn default() -> Self {
        ImageFormat::RgbaUnorm
    }
}

impl From<ImageFormat> for gfx::format::Format {
    fn from(format: ImageFormat) -> Self {
        use gfx::format::Format;

        match format {
            ImageFormat::RUnorm => Format::R8Unorm,
            ImageFormat::RgUnorm => Format::Rg8Unorm,
            ImageFormat::RgbUnorm => Format::Rgb8Unorm,
            ImageFormat::RgbaUnorm => Format::Rgba8Unorm,

            ImageFormat::Rgba32Float => Format::Rgba32Sfloat,

            ImageFormat::E5b9g9r9Float => Format::E5b9g9r9Ufloat,

            ImageFormat::D32Float => Format::D32Sfloat,
            ImageFormat::D32FloatS8Uint => Format::D32SfloatS8Uint,
        }
    }
}

impl Into<ImageFormat> for gfx::format::Format {
    fn into(self) -> ImageFormat {
        use gfx::format::Format;

        match self {
            Format::R8Unorm => ImageFormat::RUnorm,
            Format::Rg8Unorm => ImageFormat::RgUnorm,
            Format::Rgb8Unorm => ImageFormat::RgbUnorm,
            Format::Rgba8Unorm => ImageFormat::RgbaUnorm,

            Format::Rgba32Sfloat => ImageFormat::Rgba32Float,

            Format::E5b9g9r9Ufloat => ImageFormat::E5b9g9r9Float,

            Format::D32Sfloat => ImageFormat::D32Float,
            Format::D32SfloatS8Uint => ImageFormat::D32FloatS8Uint,

            _ => unimplemented!(),
        }
    }
}

impl ImageFormat {
    /// Determine if the given format contains a depth component
    pub fn is_depth(self) -> bool {
        match self {
            ImageFormat::D32FloatS8Uint => true,
            ImageFormat::D32Float => true,
            _ => false,
        }
    }

    /// Determine if the given format contains a stencil component
    pub fn is_stencil(self) -> bool {
        match self {
            ImageFormat::D32FloatS8Uint => true,
            _ => false,
        }
    }

    /// Determine if the given format contains both a depth and a stencil component
    pub fn is_depth_stencil(self) -> bool {
        self.is_depth() && self.is_stencil()
    }
}

/// Kind of image
///
/// Different kinds of images may contains the same "physical" data, but sampling might be
/// different between kinds (for example, an array of 2-dimensional images is sampled differently
/// than a 3D image in regards to mipmaps)
#[repr(u8)]
#[derive(Copy, Clone, Debug)]
pub enum ViewKind {
    /// One dimensional (N x 1 x 1)
    D1,
    /// An array of one dimensional images ((N x 1 x 1) x L)
    D1Array,
    /// Two dimensional (N x M x 1)
    D2,
    /// An array of two dimensional images ((N x M x 1) x L)
    D2Array,
    /// Three dimensional (N x M x K)
    D3,
    /// N sides using two-dimensional images (N x M) x Sides
    Cube,
    /// An array of "N-sided" two-dimensional images ((N x M) x Sides) x L)
    CubeArray,
}

impl Default for ViewKind {
    fn default() -> Self {
        ViewKind::D2
    }
}

impl From<ViewKind> for gfx::image::ViewKind {
    fn from(kind: ViewKind) -> Self {
        use gfx::image::ViewKind as vk;
        match kind {
            ViewKind::D1 => vk::D1,
            ViewKind::D1Array => vk::D1Array,
            ViewKind::D2 => vk::D2,
            ViewKind::D2Array => vk::D2Array,
            ViewKind::D3 => vk::D3,
            ViewKind::Cube => vk::Cube,
            ViewKind::CubeArray => vk::CubeArray,
        }
    }
}

pub(crate) type ImageType = AllocImage;
pub(crate) type ImageView = <back::Backend as gfx::Backend>::ImageView;

/// Image objects can hold 1 to 3-dimensional data.
///
/// The most common use case is for storing color information ("pictures"), but technically
/// any kind of data can be stored.
pub struct Image {
    pub(crate) image: ImageType,
    pub(crate) aspect: gfx::format::Aspects,
    pub(crate) view: ImageView,
    pub(crate) dimension: ImageDimension,
    pub(crate) format: gfx::format::Format,
    pub(crate) usage: gfx::image::Usage,
}

/// Errors that can occur while operating on image resources.
#[allow(missing_docs)]
#[derive(Debug, Display, From)]
pub enum ImageError {
    #[display(fmt = "The specified image handle was invalid")]
    HandleInvalid,

    #[display(fmt = "The data provided for uploading was not valid")]
    UploadDataInvalid,

    #[display(fmt = "Failed to allocate image")]
    CantCreate(AllocatorError),

    #[display(fmt = "Failed to map memory")]
    MappingError(gfx::mapping::Error),

    #[display(fmt = "Image View could not be created")]
    ViewError(gfx::image::ViewError),

    #[display(fmt = "Image can not be used a transfer destination")]
    CantWriteToImage,
}

impl std::error::Error for ImageError {}

pub(crate) struct ImageStorage {
    // TODO handle host visible images??
    transfer_dst: BTreeSet<usize>,

    storage: Storage<Image>,
}

impl ImageStorage {
    pub(crate) fn new() -> Self {
        ImageStorage {
            transfer_dst: BTreeSet::new(),
            storage: Storage::new(),
        }
    }

    pub(crate) unsafe fn release(self, device: &DeviceContext) {
        let mut alloc = device.allocator();

        for (_, image) in self.storage {
            alloc.destroy_image(&device.device, image.image);
            device.device.destroy_image_view(image.view);
        }
    }

    pub(crate) unsafe fn create<T: Into<gfx::image::Usage> + Clone>(
        &mut self,
        device: &DeviceContext,
        create_info: ImageCreateInfo<T>,
    ) -> Result<ImageHandle, ImageError> {
        use gfx::format::Format;

        let mut allocator = device.allocator();

        let format = create_info.format.into();

        // some formats are not supported on most GPUs, for example most 24 bit ones.
        // TODO: this should not use hardcoded values but values from the device info maybe?
        let format = match format {
            Format::Rgb8Unorm => Format::Rgba8Unorm,
            format => format,
        };

        let aspect = {
            let mut aspect = gfx::format::Aspects::empty();

            if format.is_depth() {
                aspect |= gfx::format::Aspects::DEPTH;
            }

            if format.is_stencil() {
                aspect |= gfx::format::Aspects::STENCIL;
            }

            if format.is_color() {
                aspect |= gfx::format::Aspects::COLOR;
            }

            aspect
        };

        let (image, usage) = {
            let image_kind = match create_info.dimension {
                ImageDimension::D1 { x } => image::Kind::D1(x, create_info.num_layers),
                ImageDimension::D2 { x, y } => {
                    image::Kind::D2(x, y, create_info.num_layers, create_info.num_samples)
                }
                ImageDimension::D3 { x, y, z } => image::Kind::D3(x, y, z),
            };

            use gfx::memory::Properties;

            let usage_flags = create_info.usage.clone().into();

            let req = ImageRequest {
                transient: create_info.is_transient,
                properties: Properties::DEVICE_LOCAL,
                kind: image_kind,
                level: 1,
                format,
                tiling: image::Tiling::Optimal,
                usage: usage_flags,
                view_caps: image::ViewCapabilities::empty(),
            };

            let image = allocator.create_image(&device.device, req)?;

            (image, usage_flags)
        };

        let image_view = device.device.create_image_view(
            image.raw(),
            create_info.kind.into(),
            format,
            create_info.swizzle.into(),
            image::SubresourceRange {
                aspects: aspect,
                layers: 0..1,
                levels: 0..1,
            },
        )?;

        let img_store = Image {
            image,
            format,
            usage,
            aspect,
            dimension: create_info.dimension,
            view: image_view,
        };

        let handle = self.storage.insert(img_store);

        if usage.contains(gfx::image::Usage::TRANSFER_DST) {
            self.transfer_dst.insert(handle.id());
        }

        Ok(handle)
    }

    pub(crate) unsafe fn upload_data(
        &self,
        device: &DeviceContext,
        sync: &mut QueueSyncRefs,
        cmd_pool: &CommandPoolTransfer,
        handle: ImageHandle,
        data: ImageUploadInfo,
    ) -> Result<(), ImageError> {
        use gfx::memory::Properties;
        use gfx::PhysicalDevice;

        let image = self.storage.get(handle).ok_or(ImageError::HandleInvalid)?;

        if !self.transfer_dst.contains(&handle.id()) {
            return Err(ImageError::CantWriteToImage);
        }

        let limits: gfx::Limits = device.adapter.physical_device.limits();

        let mut allocator = device.allocator();

        let dimensions = image.dimension;

        let upload_data_fits = {
            use self::ImageDimension as I;
            match (dimensions, data.dimension) {
                (I::D1 { x: dx }, I::D1 { x: sx }) => (sx + data.target_offset.0) <= dx,
                (I::D2 { x: dx, y: dy }, I::D2 { x: sx, y: sy }) => {
                    (sx + data.target_offset.0) <= dx && (sy + data.target_offset.1) <= dy
                }
                (
                    I::D3 {
                        x: dx,
                        y: dy,
                        z: dz,
                    },
                    I::D3 {
                        x: sx,
                        y: sy,
                        z: sz,
                    },
                ) => {
                    (sx + data.target_offset.0) <= dx
                        && (sy + data.target_offset.1) <= dy
                        && (sz + data.target_offset.2) <= dz
                }
                _ => false,
            }
        };

        if !upload_data_fits {
            return Err(ImageError::UploadDataInvalid);
        }

        let (upload_width, upload_height) = match data.dimension {
            ImageDimension::D1 { x } => (x, 1),
            ImageDimension::D2 { x, y } => (x, y),
            ImageDimension::D3 { .. } => {
                // TODO support 3D data?
                return Err(ImageError::UploadDataInvalid);
            }
        };

        let upload_nums = {
            let row_align = limits.optimal_buffer_copy_pitch_alignment as u32;
            image_copy_buffer_size(row_align, &data, (upload_width, upload_height))
        };
        let (upload_size, row_pitch, texel_size) = upload_nums;

        debug_assert!(
            upload_size >= u64::from(upload_width) * u64::from(upload_height) * texel_size as u64
        );

        let buf_req = BufferRequest {
            transient: true,
            properties: Properties::CPU_VISIBLE | Properties::COHERENT,
            usage: gfx::buffer::Usage::TRANSFER_SRC | gfx::buffer::Usage::TRANSFER_DST,
            size: upload_size,
        };

        let mut staging = allocator.create_buffer(&device.device, buf_req)?;

        // write to staging buffer
        {
            use rendy_memory::Block;

            let block = staging.block_mut();

            let range = 0..block.size();

            let mut map = block.map(&device.device, range.clone())?;

            {
                use rendy_memory::Write;

                let mut writer = map.write(&device.device, range)?;

                let slice = writer.slice();

                // Alignment strikes back again! We do copy all the rows, but the row length in the
                // staging buffer might be bigger than in the upload data, so we need to construct
                // a slice for each row instead of just copying *everything*
                for y in 0..upload_height as usize {
                    let src_start = y * (upload_width as usize) * texel_size;
                    let src_end = (y + 1) * (upload_width as usize) * texel_size;

                    let row = &data.data[src_start..src_end];

                    let dst_start = y * row_pitch as usize;
                    let dst_end = dst_start + row.len();

                    slice[dst_start..dst_end].copy_from_slice(row);
                }
            }

            block.unmap(&device.device);
        }

        // create image upload data

        use crate::transfer::BufferImageTransfer;

        let transfer_data = BufferImageTransfer {
            src: &staging,
            dst: &image.image,
            subresource_range: gfx::image::SubresourceRange {
                aspects: gfx::format::Aspects::COLOR,
                levels: 0..1,
                layers: 0..1,
            },
            copy_information: gfx::command::BufferImageCopy {
                buffer_offset: 0,
                buffer_width: row_pitch / (texel_size as u32),
                buffer_height: upload_height,
                image_layers: gfx::image::SubresourceLayers {
                    aspects: gfx::format::Aspects::COLOR,
                    level: 0,
                    layers: 0..1,
                },
                image_offset: image::Offset {
                    x: data.target_offset.0 as i32,
                    y: data.target_offset.1 as i32,
                    z: data.target_offset.2 as i32,
                },
                image_extent: image::Extent {
                    width: upload_width,
                    height: upload_height,
                    depth: 1,
                },
            },
        };

        transfer::copy_buffers_to_images(
            device,
            sync.sem_pool,
            sync.sem_list,
            cmd_pool,
            &[transfer_data],
        );

        sync.res_list.queue_buffer(staging);

        Ok(())
    }

    pub(crate) fn raw(&self, image: ImageHandle) -> Option<&Image> {
        if self.storage.is_alive(image) {
            Some(&self.storage[image])
        } else {
            None
        }
    }

    pub(crate) fn format(&self, image: ImageHandle) -> Option<gfx::format::Format> {
        self.storage.get(image).map(|img| img.format)
    }

    pub(crate) fn usage(&self, image: ImageHandle) -> Option<gfx::image::Usage> {
        self.storage.get(image).map(|img| img.usage)
    }

    pub fn destroy<I>(&mut self, res_list: &mut ResourceList, handles: I)
    where
        I: IntoIterator,
        I::Item: std::borrow::Borrow<ImageHandle>,
    {
        for handle in handles.into_iter() {
            let handle = *handle.borrow();
            if let Some(image) = self.storage.remove(handle) {
                res_list.queue_image(image.image);
                res_list.queue_image_view(image.view);

                if self.transfer_dst.contains(&handle.id()) {
                    self.transfer_dst.remove(&handle.id());
                }
            }
        }
    }
}

/// Compute the total size in bytes and the row stride
/// for a buffer that should be used to copy data into an image.
fn image_copy_buffer_size(
    row_align: u32,
    upload_info: &ImageUploadInfo,
    (width, height): (u32, u32),
) -> (u64, u32, usize) {
    let texel_size = upload_info.data.len() / (width * height) as usize;

    // Because low level graphics are low level, we need to take care about buffer
    // alignment here.
    //
    // For example an RGBA8 image with 11 * 11 dims
    // has
    //  - "texel_size" of 4 (4 components (rgba) with 1 byte size)
    //  - "width" of 11
    //  - "height" of 10
    //
    // If we want to make a buffer used for copying the image, the "row size" is important
    // since graphics APIs like to have a certain *alignment* for copying the data.
    //
    // Let's assume the "row alignment" is 8, that means each row size has to be divisible
    // by 8. In the RGBA8 example, each row has a size of `width * stride = 44`, which is
    // not divisible evenly by 8, so we need to add some padding.
    // In this case the padding we add needs to be 4 bytes, so we get to a row width of 48.
    //
    // Generally this padding is there because it seems like GPUs like it when
    // `offset_of(x, y + 1) = offset_of(x, y) + n * alignment`
    // (I strongly assume that that's because of SIMD operations)
    //

    // This mask says how many bits from the right need to be 0
    let row_alignment_mask = row_align as u32 - 1;

    // We add the alignment mask, then cut away everything stuff on the right so it's all 0s
    let row_pitch = (width * texel_size as u32 + row_alignment_mask) & !row_alignment_mask;

    let buffer_size = u64::from(height * row_pitch);

    (buffer_size, row_pitch, texel_size)
}
