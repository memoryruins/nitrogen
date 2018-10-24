
use gfx::image;
use gfx::Device;

use device::DeviceContext;

use util::storage;
use util::storage::Storage;

use types::Sampler;

#[derive(Copy, Clone)]
pub enum Filter {
    Nearest,
    Linear,
}

impl From<Filter> for image::Filter {
    fn from(filter: Filter) -> Self {
        match filter {
            Filter::Nearest => image::Filter::Nearest,
            Filter::Linear => image::Filter::Linear,
        }
    }
}

#[derive(Copy, Clone)]
pub enum WrapMode {
    Tile,
    Mirror,
    Clamp,
    Border,
}

impl From<WrapMode> for image::WrapMode {
    fn from(mode: WrapMode) -> Self {
        match mode {
            WrapMode::Tile => image::WrapMode::Tile,
            WrapMode::Mirror => image::WrapMode::Mirror,
            WrapMode::Clamp => image::WrapMode::Clamp,
            WrapMode::Border => image::WrapMode::Border,
        }
    }
}

#[derive(Copy, Clone)]
pub struct SamplerCreateInfo {
    pub min_filter: Filter,
    pub mag_filter: Filter,
    pub mip_filter: Filter,
    pub wrap_mode: (WrapMode, WrapMode, WrapMode),
}

impl From<SamplerCreateInfo> for image::SamplerInfo {
    fn from(create: SamplerCreateInfo) -> Self {
        image::SamplerInfo {
            min_filter: create.min_filter.into(),
            mag_filter: create.mag_filter.into(),
            mip_filter: create.mip_filter.into(),
            wrap_mode: (
                create.wrap_mode.0.into(),
                create.wrap_mode.1.into(),
                create.wrap_mode.2.into(),
            ),
            lod_bias: 0.0.into(),
            lod_range: (0.0.into())..(1.0.into()),
            comparison: None,
            border: image::PackedColor(0x0),
            anisotropic: image::Anisotropic::Off,
        }
    }
}

pub type SamplerHandle = storage::Handle<Sampler>;

pub struct SamplerStorage {
    pub storage: Storage<Sampler>,
}

impl SamplerStorage {
    pub fn new() -> Self {
        Self {
            storage: Storage::new(),
        }
    }

    pub fn create(
        &mut self,
        device: &DeviceContext,
        create_infos: &[SamplerCreateInfo],
    ) -> Vec<SamplerHandle> {

        let mut results = Vec::with_capacity(create_infos.len());

        for create_info in create_infos {

            let create_info = create_info.clone().into();

            let sampler = { device.device.create_sampler(create_info) };

            let (handle, _) = self.storage.insert(sampler);

            results.push(handle);
        }

        results
    }

    pub fn raw(&self, sampler: SamplerHandle) -> Option<&Sampler> {
        if self.storage.is_alive(sampler) {
            Some(&self.storage[sampler])
        } else {
            None
        }
    }

    pub fn destroy(&mut self, device: &DeviceContext, handles: &[SamplerHandle]) {
        for handle in handles {
            match self.storage.remove(*handle) {
                Some(sampler) => {
                    device.device.destroy_sampler(sampler);
                },
                None => {}
            }
        }
    }
}
