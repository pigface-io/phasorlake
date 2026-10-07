-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#NEGATIVE_RESIDUE#FILE01#202408.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS NEGATIVE_RESIDUE owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#NEGATIVE_RESIDUE#FILE01#202408010000.CSV
APPEND INTO TABLE NEGATIVE_RESIDUE
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
SETTLEMENTDATE DATE "YYYY/MM/DD HH24:MI:SS",
NRM_DATETIME DATE "YYYY/MM/DD HH24:MI:SS",
DIRECTIONAL_INTERCONNECTORID CHAR(30),
NRM_ACTIVATED_FLAG FLOAT EXTERNAL,
CUMUL_NEGRESIDUE_AMOUNT FLOAT EXTERNAL,
CUMUL_NEGRESIDUE_PREV_TI FLOAT EXTERNAL,
NEGRESIDUE_CURRENT_TI FLOAT EXTERNAL,
NEGRESIDUE_PD_NEXT_TI FLOAT EXTERNAL,
PRICE_REVISION CHAR(30),
PREDISPATCHSEQNO CHAR(20),
EVENT_ACTIVATED_DI DATE "YYYY/MM/DD HH24:MI:SS",
EVENT_DEACTIVATED_DI DATE "YYYY/MM/DD HH24:MI:SS",
DI_NOTBINDING_COUNT FLOAT EXTERNAL,
DI_VIOLATED_COUNT FLOAT EXTERNAL,
NRMCONSTRAINT_BLOCKED_FLAG FLOAT EXTERNAL)
-- End of Script
