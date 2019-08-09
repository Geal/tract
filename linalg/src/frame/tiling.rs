use std::fmt::Debug;
use std::marker::PhantomData;
use std::ops::{Add, Mul};

use num_traits::Zero;

use super::PackA;
use super::PackB;

use super::*;

#[repr(C, usize)]
#[derive(PartialEq, Clone, Debug)]
pub enum StorageSpec<T>
where
    T: Copy + Add + Mul + Zero + Debug + PartialEq + Send + Sync,
{
    Strides { ptr: *const T, row_byte_stride: isize, col_byte_stride: isize, mr: usize, nr: usize },
    Packed { ptr: *const T, panel_len: usize },
    OffsetsAndPtrs { row_byte_offsets: Vec<isize>, col_ptrs: Vec<*const T>, nr: usize },
}

impl<T> StorageSpec<T>
where
    T: Copy + Add + Mul + Zero + Debug + PartialEq + Send + Sync,
{
    unsafe fn panel_a(&self, i: usize) -> TileStorageSpec<T> {
        match self {
            StorageSpec::Packed { ptr, panel_len } => {
                TileStorageSpec::Packed { ptr: ptr.offset((panel_len * i) as isize) }
            }
            _ => unimplemented!(),
        }
    }

    unsafe fn panel_b(&self, i: usize) -> TileStorageSpec<T> {
        match self {
            StorageSpec::Packed { ptr, panel_len } => {
                TileStorageSpec::Packed { ptr: ptr.offset((panel_len * i) as isize) }
            }
            StorageSpec::OffsetsAndPtrs { row_byte_offsets, col_ptrs, nr } => {
                TileStorageSpec::OffsetsAndPtrs {
                    row_byte_offsets: row_byte_offsets.as_ptr(),
                    col_ptrs: col_ptrs.as_ptr().offset((nr * i) as isize),
                }
            }
            _ => unimplemented!(),
        }
    }

    fn tile(&self, down: usize, right: usize) -> TileStorageSpec<T> {
        match self {
            StorageSpec::Strides { ptr, row_byte_stride, col_byte_stride, mr, nr } => {
                TileStorageSpec::Strides {
                    ptr: ((*ptr as isize)
                        + (*row_byte_stride as usize * down * mr
                            + *col_byte_stride as usize * right * nr)
                            as isize) as *mut T,
                    row_byte_stride: *row_byte_stride,
                    col_byte_stride: *col_byte_stride,
                }
            }
            _ => unimplemented!(),
        }
    }

    unsafe fn set(&mut self, row: usize, col: usize, val: T) {
        match self {
            StorageSpec::Strides { ptr, row_byte_stride, col_byte_stride, .. } => {
                *(((*ptr as isize)
                    + (*row_byte_stride as usize * row + *col_byte_stride as usize * col) as isize)
                    as *mut T) = val;
            }
            _ => unimplemented!(),
        }
    }
}

pub trait Tile<T: Copy + Add + Mul + Zero + Debug>: Send + Sync + Debug + objekt::Clone
where
    T: Copy + Add + Mul + Zero + Debug + PartialEq + Send + Sync,
{
    fn a_pack(&self) -> PackA<T>;
    fn b_pack(&self) -> PackB<T>;

    fn m(&self) -> usize;
    fn k(&self) -> usize;
    fn n(&self) -> usize;

    unsafe fn a_from_packed(&self, ptr: *const T) -> StorageSpec<T>;
    unsafe fn b_from_packed(&self, ptr: *const T) -> StorageSpec<T>;

    unsafe fn b_from_data_and_offsets(
        &self,
        data: *const T,
        rows_offsets: &[isize],
        cols_offsets: &[isize],
    ) -> StorageSpec<T>;

    unsafe fn c_from_data_and_strides(
        &self,
        data: *const T,
        row_stride: isize,
        col_stride: isize,
    ) -> StorageSpec<T>;

    unsafe fn run(&self, a: &StorageSpec<T>, b: &StorageSpec<T>, c: &mut StorageSpec<T>);
}

clone_trait_object!(<T> Tile<T> where T: Copy + Add + Mul + Zero);

#[derive(Debug, Clone, new)]
pub struct TileOp<K, T>
where
    T: Copy + Add + Mul + Zero + Debug + PartialEq + Send + Sync,
    K: TilingKer<T>,
{
    pub m: usize,
    pub k: usize,
    pub n: usize,
    phantom: PhantomData<(K, T)>,
}

impl<K, T> Tile<T> for TileOp<K, T>
where
    T: Copy + Add + Mul + Zero + Debug + PartialEq + Send + Sync,
    K: TilingKer<T>,
{
    fn a_pack(&self) -> PackA<T> {
        PackA::new(self.k, self.m, K::mr(), K::alignment_bytes_packed_a())
    }

    fn b_pack(&self) -> PackB<T> {
        PackB::new(self.k, self.n, K::nr(), K::alignment_bytes_packed_b())
    }

    fn m(&self) -> usize {
        self.m
    }

    fn n(&self) -> usize {
        self.n
    }

    fn k(&self) -> usize {
        self.k
    }

    unsafe fn a_from_packed(&self, ptr: *const T) -> StorageSpec<T> {
        StorageSpec::Packed { ptr, panel_len: (self.k * K::mr()) }
    }

    unsafe fn b_from_packed(&self, ptr: *const T) -> StorageSpec<T> {
        StorageSpec::Packed { ptr, panel_len: (self.k * K::nr()) }
    }

    unsafe fn b_from_data_and_offsets(
        &self,
        data: *const T,
        rows_offsets: &[isize],
        cols_offsets: &[isize],
    ) -> StorageSpec<T> {
        let mut col_ptrs: Vec<_> = cols_offsets.iter().map(|&co| data.offset(co)).collect();
        let wanted = (col_ptrs.len() + K::nr() - 1) / K::nr() * K::nr();
        while col_ptrs.len() < wanted {
            col_ptrs.push(col_ptrs[col_ptrs.len() - 1]);
        }
        let row_byte_offsets: Vec<_> =
            rows_offsets.iter().map(|&ro| ro * std::mem::size_of::<T>() as isize).collect();
        StorageSpec::OffsetsAndPtrs { col_ptrs, row_byte_offsets, nr: K::nr() }
    }

    unsafe fn c_from_data_and_strides(
        &self,
        data: *const T,
        row_stride: isize,
        col_stride: isize,
    ) -> StorageSpec<T> {
        StorageSpec::Strides {
            ptr: data,
            row_byte_stride: row_stride * std::mem::size_of::<T>() as isize,
            col_byte_stride: col_stride * std::mem::size_of::<T>() as isize,
            mr: K::mr(),
            nr: K::nr(),
        }
    }

    unsafe fn run(&self, a: &StorageSpec<T>, b: &StorageSpec<T>, c: &mut StorageSpec<T>) {
        let mr = K::mr();
        let nr = K::nr();
        let m = self.m;
        let k = self.k;
        let n = self.n;
        let tmpc = vec![T::zero(); mr * nr];
        let ref mut tmp_tile = self.c_from_data_and_strides(tmpc.as_ptr(), nr as isize, 1);
        let linear = LinearSpec::Mul { k };
        let linear = (&linear) as *const LinearSpec;
        let non_linear = std::ptr::null();
        for ia in 0..m / mr {
            let ref a = a.panel_a(ia);
            for ib in 0..n / nr {
                let ref b = b.panel_b(ib);
                let ref tile_c = c.tile(ia, ib);
                let err = K::kernel(&TileOpSpec {
                    a: a as _,
                    b: b as _,
                    c: tile_c as _,
                    linear,
                    non_linear,
                });
                if err != 0 {
                    panic!("Kernel return error {}", err);
                }
            }
            if n % nr != 0 {
                let ref b = b.panel_b(n / nr);
                let ref tmp_tile_c = tmp_tile.tile(0, 0);
                let err = K::kernel(&TileOpSpec {
                    a: a as _,
                    b: b as _,
                    c: tmp_tile_c as _,
                    linear,
                    non_linear,
                });
                if err != 0 {
                    panic!("Kernel return error {}", err);
                }
                for y in 0..mr {
                    for x in 0..(n % nr) {
                        c.set(mr * ia + y, x + n / nr * nr, tmpc[y * nr + x])
                    }
                }
            }
        }
        if m % mr != 0 {
            let ref panel_a = a.panel_a(m / mr);
            let ref tmp_tile_c = tmp_tile.tile(0, 0);
            for ib in 0..n / nr {
                let ref b = b.panel_b(ib);
                let err = K::kernel(&TileOpSpec {
                    a: panel_a as _,
                    b: b as _,
                    c: tmp_tile_c as _,
                    linear,
                    non_linear,
                });
                if err != 0 {
                    panic!("Kernel return error {}", err);
                }
                for y in 0..(m % mr) {
                    for x in 0..nr {
                        c.set(m / mr * mr + y, x + ib * nr, tmpc[y * nr + x])
                    }
                }
            }
            if n % nr != 0 {
                let ref b = b.panel_b(n / nr);
                let err = K::kernel(&TileOpSpec {
                    a: panel_a as _,
                    b: b as _,
                    c: tmp_tile_c as _,
                    linear,
                    non_linear,
                });
                if err != 0 {
                    panic!("Kernel return error {}", err);
                }
                for y in 0..(m % mr) {
                    for x in 0..(n % nr) {
                        c.set(m / mr * mr + y, x + n / nr * nr, tmpc[y * nr + x])
                    }
                }
            }
        }
    }
}

#[cfg(test)]
#[macro_use]
pub mod test {
    use super::*;
    use crate::align;
    use proptest::prelude::*;
    use proptest::test_runner::TestCaseResult;

    pub fn check_close(found: &[f32], expected: &[f32]) -> TestCaseResult {
        proptest::prop_assert!(
            found.iter().zip(expected.iter()).all(|(a, b)| (a - b).abs() < 0.001),
            "found: {:?} expected: {:?}",
            found,
            expected
        );
        Ok(())
    }

    #[macro_export]
    macro_rules! tile_frame_tests {
        ($cond:expr, $ker:ty) => {
            mod frame {
                #[allow(unused_imports)]
                use crate::frame::tiling::test::*;
                proptest::proptest! {
                    #[test]
                    fn mat_mul_prepacked((m, k, n, ref a, ref b) in strat_mat_mul()) {
                        if $cond {
                            test_mat_mul_prep_f32::<$ker>(m, k, n, a, b)?
                        }
                    }

                    #[test]
                    fn conv_prepacked(pb in strat_conv_1d()) {
                        if $cond {
                            check_close(&*pb.run::<$ker>(), &*pb.expected())?;
                        }
                    }
                }

                #[test]
                fn mat_mul_1() {
                    if $cond {
                        test_mat_mul_prep_f32::<$ker>(
                            3,
                            4,
                            2,
                            &[-3.0, 3.0, 5.0, -5.0, 6.0, 0.0, -6.0, -5.0, 0.0, 0.0, 9.0, 7.0],
                            &[-8.0, 5.0, 5.0, -3.0, 5.0, 7.0, -8.0, -1.0],
                        )
                        .unwrap()
                    }
                }

                #[test]
                fn conv_prepacked_1() {
                    if $cond {
                        let mut filters = vec![0.0f32; 3 * 14 * 2];
                        filters[13 * 6 + 5] = 1.0;
                        let mut data = vec![0.0f32; 3 * 10];
                        data[8 + 2 * 10] = 1.0; // last used input
                        let pb = ConvProblem {
                            ci: 3,
                            co: 14,
                            kt: 2,
                            stride: 3,
                            dilation: 2,
                            filters,
                            data,
                        };
                        check_close(&*pb.run::<$ker>(), &*pb.expected()).unwrap();
                    }
                }
            }
        };
    }

    pub fn strat_mat_mul() -> BoxedStrategy<(usize, usize, usize, Vec<f32>, Vec<f32>)> {
        (1usize..5, 1usize..5, 1usize..5)
            .prop_flat_map(move |(m, k, n)| {
                (
                    Just(m),
                    Just(k),
                    Just(n),
                    proptest::collection::vec((-10..10).prop_map(|a| a as f32), m * k),
                    proptest::collection::vec((-10..10).prop_map(|a| a as f32), n * k),
                )
            })
            .boxed()
    }

    pub fn test_mat_mul_prep_f32<K: TilingKer<f32>>(
        m: usize,
        k: usize,
        n: usize,
        a: &[f32],
        b: &[f32],
    ) -> Result<(), proptest::test_runner::TestCaseError> {
        let op = TileOp::<K, f32>::new(m, k, n);
        unsafe {
            let mut packed_a: Vec<f32> =
                align::uninitialized(op.a_pack().len(), op.a_pack().alignment());
            op.a_pack().pack(packed_a.as_mut_ptr(), a.as_ptr(), k as isize, 1);

            let mut packed_b: Vec<f32> =
                align::uninitialized(op.b_pack().len(), op.b_pack().alignment());
            op.b_pack().pack(packed_b.as_mut_ptr(), b.as_ptr(), n as isize, 1);

            let mut found = vec![9999.0f32; m * n];

            op.run(
                &op.a_from_packed(packed_a.as_ptr()),
                &op.b_from_packed(packed_b.as_ptr()),
                &mut op.c_from_data_and_strides(found.as_mut_ptr(), n as isize, 1),
            );

            let mut expected = vec![0.0f32; m * n];
            for x in 0..n {
                for y in 0..m {
                    for i in 0..k {
                        expected[x + y * n] += a[i + k * y] * b[x + i * n]
                    }
                }
            }

            proptest::prop_assert!(
                found.iter().zip(expected.iter()).all(|(a, b)| (a - b).abs() < 0.001),
                "found: {:?} expected: {:?}",
                found,
                expected
            );
        }
        Ok(())
    }

    #[derive(Clone, Debug)]
    pub struct ConvProblem {
        pub ci: usize,
        pub co: usize,
        pub kt: usize,
        pub stride: usize,
        pub dilation: usize,
        pub filters: Vec<f32>,
        pub data: Vec<f32>,
    }

    impl ConvProblem {
        pub fn kernel_field(&self) -> usize {
            self.dilation * (self.kt - 1) + 1
        }
        // this is not n, but the T in NTC of input to direct convolution
        pub fn input_width(&self) -> usize {
            assert!(self.data.len() % self.ci == 0);
            self.data.len() / self.ci
        }
        pub fn output_width(&self) -> usize {
            (self.input_width() - self.kernel_field()) / self.stride + 1
        }
        pub fn m(&self) -> usize {
            self.co
        }
        pub fn k(&self) -> usize {
            self.ci * self.kt
        }
        pub fn n(&self) -> usize {
            self.output_width()
        }
        pub fn data_cols_offsets(&self) -> Vec<isize> {
            (0..self.output_width()).map(|i| (i * self.stride) as isize).collect()
        }
        pub fn data_rows_offsets(&self) -> Vec<isize> {
            (0..self.ci)
                .flat_map(move |ici| {
                    (0..self.kt)
                        .map(move |ikt| (ikt * self.dilation + ici * self.input_width()) as isize)
                })
                .collect()
        }
        pub fn expected(&self) -> Vec<f32> {
            let mut expect = vec![0.0f32; self.co * self.output_width()];
            for x in 0..self.output_width() {
                for ico in 0..self.co {
                    for ikt in 0..self.kt {
                        for ici in 0..self.ci {
                            let f = self.filters[ici * self.kt + ikt + self.ci * self.kt * ico];
                            let d = self.data
                                [x * self.stride + ikt * self.dilation + ici * self.input_width()];
                            expect[x + ico * self.output_width()] += f * d;
                        }
                    }
                }
            }
            expect
        }

        pub fn run<K: TilingKer<f32>>(&self) -> Vec<f32> {
            let op = TileOp::<K, f32>::new(self.m(), self.k(), self.n());
            unsafe {
                let mut packed_a: Vec<f32> =
                    align::uninitialized(op.a_pack().len(), op.a_pack().alignment());
                op.a_pack().pack(
                    packed_a.as_mut_ptr(),
                    self.filters.as_ptr(),
                    self.k() as isize,
                    1,
                );

                let mut found = vec![9999.0f32; self.co * self.output_width()];
                op.run(
                    &op.a_from_packed(packed_a.as_ptr()),
                    &op.b_from_data_and_offsets(
                        self.data.as_ptr(),
                        &self.data_rows_offsets(),
                        &self.data_cols_offsets(),
                    ),
                    &mut op.c_from_data_and_strides(found.as_mut_ptr(), self.n() as isize, 1),
                );
                found
            }
        }
    }

    pub fn strat_conv_1d() -> BoxedStrategy<ConvProblem> {
        (1usize..40, 1usize..40, 1usize..10, 1usize..5, 1usize..5)
            .prop_flat_map(|(ci, co, kt, stride, dilation)| {
                let min = ((kt - 1) * dilation + 1) * stride;
                (Just(ci), Just(co), Just(kt), Just(stride), Just(dilation), min..min + 10)
            })
            .prop_flat_map(move |(ci, co, kt, stride, dilation, t)| {
                (
                    Just(ci),
                    Just(co),
                    Just(kt),
                    Just(stride),
                    Just(dilation),
                    proptest::collection::vec((-10..10).prop_map(|a| a as f32), ci * co * kt),
                    proptest::collection::vec((-10..10).prop_map(|a| a as f32), t * ci),
                )
            })
            .prop_map(move |(ci, co, kt, stride, dilation, filters, data)| ConvProblem {
                ci,
                co,
                kt,
                stride,
                dilation,
                filters,
                data,
            })
            .boxed()
    }
}