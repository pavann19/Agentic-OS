#include "interrupts.h"
#include "idt.h"
#include "print.h"
#include "pic.h"
#include "keyboard.h"
#include "klog.h"
#include "graphics.h"

// GCC specific attribute to generate a proper interrupt handler (iretq)
__attribute__((interrupt)) void DivideByZero_Handler(InterruptFrame* frame) {
    klog_error("EXCEPTION_DIVIDE_BY_ZERO rip=%x", frame->rip);
    set_text_color(0x00FFFFFF, 0x00FF0000); // White on Red (BSOD!)
    printf("\n*** FATAL SYSTEM ERROR ***\n");
    printf("Exception 0: DIVIDE BY ZERO\n");
    printf("RIP: 0x%x\n", frame->rip);
    printf("CS: 0x%x RFLAGS: 0x%x\n", frame->cs, frame->rflags);
    printf("RSP: 0x%x SS: 0x%x\n", frame->rsp, frame->ss);
    printf("System Halted.\n");
    swap_buffers();
    while(1) { __asm__ __volatile__("hlt"); }
}

__attribute__((interrupt)) void PageFault_Handler(InterruptFrame* frame, uint64_t error_code) {
    set_text_color(0x00FFFFFF, 0x00FF0000); 
    printf("\n*** FATAL SYSTEM ERROR ***\n");
    printf("Exception 14: PAGE FAULT (Error Code: %d)\n", error_code);
    
    // Get the memory address that caused the fault from CR2
    uint64_t cr2;
    __asm__ __volatile__("mov %%cr2, %0" : "=r" (cr2));
    klog_error("EXCEPTION_PAGE_FAULT rip=%x cr2=%x code=%u", frame->rip, cr2, error_code);
    printf("Faulting Address: 0x%x\n", cr2);

    printf("RIP: 0x%x\n", frame->rip);
    printf("System Halted.\n");
    while(1) { __asm__ __volatile__("hlt"); }
}

__attribute__((interrupt)) void GPF_Handler(InterruptFrame* frame, uint64_t error_code) {
    klog_error("EXCEPTION_GPF rip=%x code=%u", frame->rip, error_code);
    set_text_color(0x00FFFFFF, 0x00FF0000); 
    printf("\n*** FATAL SYSTEM ERROR ***\n");
    printf("Exception 13: GENERAL PROTECTION FAULT (Error Code: %d)\n", error_code);
    printf("RIP: 0x%x\n", frame->rip);
    printf("System Halted.\n");
    while(1) { __asm__ __volatile__("hlt"); }
}

void interrupts_init(void) {
    // Basic CPU Exceptions
    idt_set_descriptor(0, (void*)DivideByZero_Handler, 0x8E);
    idt_set_descriptor(13, (void*)GPF_Handler, 0x8E);
    idt_set_descriptor(14, (void*)PageFault_Handler, 0x8E);
    
    // Remap PIC to offsets 0x20 (32) and 0x28 (40)
    pic_remap(0x20, 0x28);
    
    // Register Keyboard Handler at 0x21 (IRQ1)
    idt_set_descriptor(0x21, (void*)Keyboard_Handler, 0x8E);
    
    // Unmask the Keyboard IRQ
    pic_unmask(1);

    printf("Basic Exception Handlers Registered.\n");
    printf("Hardware Interrupts (PIC) Initialized.\n");
}
