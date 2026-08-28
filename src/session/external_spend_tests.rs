// Copyright 2026 Muvon Un Limited
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::*;

#[test]
#[serial_test::serial]
fn zero_and_negative_costs_are_no_ops() {
	take(); // drain to a known state
	record(0.0);
	record(-1.0);
	record(-0.001);
	assert_eq!(take(), 0.0);
}

#[test]
#[serial_test::serial]
fn take_on_empty_accumulator_returns_zero() {
	take();
	assert_eq!(take(), 0.0);
}

#[test]
#[serial_test::serial]
fn record_then_take_returns_amount() {
	take();
	record(1.5);
	assert_eq!(take(), 1.5);
}

#[test]
#[serial_test::serial]
fn multiple_records_sum() {
	take();
	record(0.25);
	record(0.75);
	record(2.0);
	assert_eq!(take(), 3.0);
}

#[test]
#[serial_test::serial]
fn take_drains_accumulator() {
	take();
	record(2.5);
	assert_eq!(take(), 2.5);
	assert_eq!(take(), 0.0);
}
