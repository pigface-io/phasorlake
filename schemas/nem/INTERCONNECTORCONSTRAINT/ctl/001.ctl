-- ************************************************************ 
-- Title:	PUBLIC_DVD_INTERCONNECTORCONSTRAINT_201501.ctl 
-- ************************************************************  
-- Project : NEMMCO MONTHLY DVD RELEASE 
-- Purpose : Control File to load monthly csv file 
-- Target Application  : Monthly DVD Release 
-- Application Version : 1 
-- Visibility Id : PUBLIC 
-- 
-- ************************************************************ 
-- Special Instructions : 1. USER TO CONNECT AS INTERCONNECTORCONSTRAINT owner 
-- 
-- ************************************************************ 
-- History : 
--      MMS Data Model Version   Template Generated On	
--      ======================   ======================	
--      PUBLIC			       v4.23	
-- ************************************************************ 
LOAD DATA 
INFILE PUBLIC_DVD_INTERCONNECTORCONSTRAINT_201501010000.csv  
APPEND INTO TABLE INTERCONNECTORCONSTRAINT  
WHEN (1:1) = 'D'  
FIELDS TERMINATED BY ','  
OPTIONALLY ENCLOSED BY '"'  
TRAILING NULLCOLS	
(row_type FILLER, 
report_type FILLER, 
report_subtype FILLER, 
report_version FILLER, 
RESERVEOVERALLLOADFACTOR FLOAT EXTERNAL, 
FROMREGIONLOSSSHARE FLOAT EXTERNAL, 
EFFECTIVEDATE DATE "yyyy/mm/dd hh24:mi:ss", 
VERSIONNO FLOAT EXTERNAL, 
INTERCONNECTORID CHAR(10), 
MAXMWIN FLOAT EXTERNAL, 
MAXMWOUT FLOAT EXTERNAL, 
LOSSCONSTANT FLOAT EXTERNAL, 
LOSSFLOWCOEFFICIENT FLOAT EXTERNAL, 
EMSMEASURAND CHAR(40), 
AUTHORISEDBY CHAR(15), 
AUTHORISEDDATE DATE "yyyy/mm/dd hh24:mi:ss", 
DYNAMICRHS CHAR(1), 
IMPORTLIMIT FLOAT EXTERNAL, 
EXPORTLIMIT FLOAT EXTERNAL, 
OUTAGEDERATIONFACTOR FLOAT EXTERNAL, 
NONPHYSICALLOSSFACTOR FLOAT EXTERNAL, 
OVERLOADFACTOR60SEC FLOAT EXTERNAL, 
OVERLOADFACTOR6SEC FLOAT EXTERNAL, 
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss", 
FCASSUPPORTUNAVAILABLE FLOAT EXTERNAL, 
ICTYPE CHAR(10)) 
-- End of Script 
