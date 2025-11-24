package condition

// This file is originally from https://github.com/crossplane/crossplane-runtime/blob/master/apis/common/v1/condition.go

import (
	"fmt"
	"sort"
	"strings"

	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
)

// A ConditionType represents a condition a resource could be in.
// nolint
type ConditionType string

// A ConditionReason represents the reason a resource is in a condition.
// nolint
type ConditionReason string

const (
	ConditionTrue    = corev1.ConditionTrue
	ConditionFalse   = corev1.ConditionFalse
	ConditionUnknown = corev1.ConditionUnknown
)

// A Condition that may apply to a resource.
type Condition struct {
	// Type of this condition. At most one of each condition type may apply to
	// a resource at any point in time.
	Type ConditionType `json:"type"`

	// If set, this represents the .metadata.generation that the object condition was set based upon.
	// +optional
	ObservedGeneration int64 `json:"observedGeneration,omitempty"`

	// Status of this condition; is it currently True, False, or Unknown.
	Status corev1.ConditionStatus `json:"status"`

	// LastTransitionTime is the last time this condition transitioned from one
	// status to another.
	// +optional
	LastTransitionTime metav1.Time `json:"lastTransitionTime,omitempty"`

	// Last time we probed the condition.
	// +optional
	LastProbeTime metav1.Time `json:"lastProbeTime,omitempty"`

	// A Reason for this condition's last transition from one status to another.
	// +optional
	Reason ConditionReason `json:"reason,omitempty"`

	// A Message containing details about this condition's last transition from
	// one status to another, if any.
	// +optional
	Message string `json:"message,omitempty"`
}

// Equal returns true if the condition is identical to the supplied condition,
// ignoring the LastTransitionTime.
func (c Condition) Equal(other Condition) bool {
	return c.Type == other.Type &&
		c.Status == other.Status &&
		c.Reason == other.Reason &&
		c.Message == other.Message &&
		c.ObservedGeneration == other.ObservedGeneration
}

// WithMessage returns a condition by adding the provided message to existing
// condition.
func (c Condition) WithMessage(msg string) Condition {
	c.Message = msg
	return c
}

// NOTE(negz): Conditions are implemented as a slice rather than a map to comply
// with Kubernetes API conventions. Ideally we'd comply by using a map that
// marshalled to a JSON array, but doing so confuses the CRD schema generator.
// https://github.com/kubernetes/community/blob/9bf8cd/contributors/devel/sig-architecture/api-conventions.md#lists-of-named-subobjects-preferred-over-maps

// NOTE(negz): Do not manipulate Conditions directly. Use the Set method.

// A ConditionedStatus reflects the observed status of a resource. Only
// one condition of each type may exist.
type ConditionedStatus struct {
	// Conditions of the resource.
	// +optional
	Conditions []Condition `json:"conditions,omitempty"`
}

// NewConditionedStatus returns a stat with the supplied conditions set.
func NewConditionedStatus(c ...Condition) *ConditionedStatus {
	s := &ConditionedStatus{}
	s.SetConditions(c...)
	return s
}

// GetCondition returns the condition for the given ConditionType if exists,
// otherwise returns an unknown condition.
func (s *ConditionedStatus) GetCondition(ct ConditionType) Condition {
	for _, c := range s.Conditions {
		if c.Type == ct {
			return c
		}
	}

	return Condition{Type: ct, Status: ConditionUnknown}
}

// SetConditions sets the supplied conditions, replacing any existing conditions
// of the same type. This is a no-op if all supplied conditions are identical,
// ignoring the last transition time, to those already set.
func (s *ConditionedStatus) SetConditions(c ...Condition) {
	for _, new := range c {
		exists := false
		for i, existing := range s.Conditions {
			if existing.Type != new.Type {
				continue
			}

			if existing.Equal(new) {
				exists = true
				continue
			}

			s.Conditions[i] = new
			exists = true
		}
		if !exists {
			s.Conditions = append(s.Conditions, new)
		}
	}
}

// Equal returns true if the status is identical to the supplied status,
// ignoring the LastTransitionTimes and order of statuses.
func (s *ConditionedStatus) Equal(other *ConditionedStatus) bool {
	if s == nil || other == nil {
		return s == nil && other == nil
	}

	if len(other.Conditions) != len(s.Conditions) {
		return false
	}

	sc := make([]Condition, len(s.Conditions))
	copy(sc, s.Conditions)

	oc := make([]Condition, len(other.Conditions))
	copy(oc, other.Conditions)

	// We should not have more than one condition of each type.
	sort.Slice(sc, func(i, j int) bool { return sc[i].Type < sc[j].Type })
	sort.Slice(oc, func(i, j int) bool { return oc[i].Type < oc[j].Type })

	for i := range sc {
		if !sc[i].Equal(oc[i]) {
			return false
		}
	}

	return true
}

// AnyUnknown returns true if any of the supplied condition types are unknown.
func (s *ConditionedStatus) AnyUnknown(condTypes ...ConditionType) bool {
	for _, condType := range condTypes {
		if s.GetCondition(condType).Status == ConditionUnknown {
			return true
		}
	}
	return false
}

// IsAvailable returns true if the resource is available.
func (s *ConditionedStatus) IsAvailable() bool {
	return s.AllTrue(TypeReady)
}

// AllTrue returns true if all of the supplied condition types are true.
func (s *ConditionedStatus) AllTrue(condTypes ...ConditionType) bool {
	if err := s.ErrAllTrue(condTypes...); err != nil {
		return false
	}

	return true
}

// ErrAllTrue returns an error containing all of the supplied condition types that are NOT true.
// All conditions must have the same ObservedGeneration.
func (s *ConditionedStatus) ErrAllTrue(condTypes ...ConditionType) error {
	var errs []string
	var observedGeneration int64

	for _, condType := range condTypes {
		cond := s.GetCondition(condType)
		if cond.Status != ConditionTrue {
			errs = append(errs, string(condType))
			continue
		}
		if cond.ObservedGeneration == 0 {
			// if we got this far, the condition is 'True' but has no observed generation
			// meaning this condition doesn't track generation
			continue
		}
		// found the first condition with an observed generation
		if observedGeneration == 0 {
			observedGeneration = cond.ObservedGeneration
		}
		if cond.ObservedGeneration != observedGeneration {
			errs = append(errs, string(condType))
		}
	}
	if len(errs) == 0 {
		return nil
	}
	return fmt.Errorf("pending conditions: %s", strings.Join(errs, ", "))
}
