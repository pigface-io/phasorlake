-- ************************************************************
-- Title:	PUBLIC_DVD_PREDISPATCHCONSTRAINT_D_202401.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS PREDISPATCHCONSTRAINT owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_DVD_PREDISPATCHCONSTRAINT_D_202401010000.CSV
APPEND INTO TABLE PREDISPATCHCONSTRAINT
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
PREDISPATCHSEQNO CHAR(20),
RUNNO FLOAT EXTERNAL,
CONSTRAINTID CHAR(20),
PERIODID CHAR(20),
INTERVENTION FLOAT EXTERNAL,
RHS FLOAT EXTERNAL,
MARGINALVALUE FLOAT EXTERNAL,
VIOLATIONDEGREE FLOAT EXTERNAL,
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss",
DATETIME DATE "yyyy/mm/dd hh24:mi:ss",
CONFIDENTIAL_TO FILLER,
GENCONID_EFFECTIVEDATE DATE "yyyy/mm/dd hh24:mi:ss",
GENCONID_VERSIONNO FLOAT EXTERNAL,
LHS FLOAT EXTERNAL)
-- End of Script
